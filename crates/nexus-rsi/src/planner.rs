//! Improvement planning.
//!
//! The planner turns recurring, structured evidence (the [`HarnessEvent`]s the
//! [`crate::ObservationCollector`] recorded) into typed
//! [`ImprovementProposal`] candidates in the harness registry. It never applies
//! anything — a candidate enters at `Observed` and must walk the full WARP path.
//! One signal below the recurrence threshold produces nothing, so a single bad
//! turn cannot trigger a self-change.

use nexus_core::harness::{
    EvidenceReference, HarnessEvent, HarnessRepository, ImprovementCategory, ImprovementProposal,
    ImprovementStatus, ImprovementTarget, RiskTier, SuccessMetric,
};
use nexus_core::Result;
use std::collections::BTreeMap;

use crate::event_type;

/// Turns recurring evidence into governed candidates.
pub struct ImprovementPlanner {
    /// How many times a signal must recur before it becomes a candidate.
    min_occurrences: usize,
}

impl Default for ImprovementPlanner {
    fn default() -> Self {
        // Matches the long-standing "repeated 3×" heuristic already used for
        // workflow detection, so behaviour is familiar.
        Self { min_occurrences: 3 }
    }
}

impl ImprovementPlanner {
    pub fn new(min_occurrences: usize) -> Self {
        Self {
            min_occurrences: min_occurrences.max(1),
        }
    }

    /// Analyse events and return candidate proposals **without persisting** them.
    pub fn plan(&self, events: &[HarnessEvent]) -> Vec<ImprovementProposal> {
        let mut candidates = Vec::new();
        candidates.extend(self.tool_failure_candidates(events));
        candidates.extend(self.repeated_workflow_candidates(events));
        candidates.extend(self.context_overflow_candidates(events));
        candidates
    }

    /// Analyse events and persist the resulting candidates, returning their ids.
    pub fn plan_and_record(
        &self,
        repo: &HarnessRepository,
        events: &[HarnessEvent],
    ) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for candidate in self.plan(events) {
            repo.save_improvement(&candidate)?;
            ids.push(candidate.id);
        }
        Ok(ids)
    }

    fn tool_failure_candidates(&self, events: &[HarnessEvent]) -> Vec<ImprovementProposal> {
        let mut by_tool: BTreeMap<String, Vec<&HarnessEvent>> = BTreeMap::new();
        for event in events
            .iter()
            .filter(|e| e.event_type == event_type::TOOL_FAILURE)
        {
            if let Some(tool) = event.metadata.get("tool").and_then(|v| v.as_str()) {
                by_tool.entry(tool.to_string()).or_default().push(event);
            }
        }
        by_tool
            .into_iter()
            .filter(|(_, hits)| hits.len() >= self.min_occurrences)
            .filter_map(|(tool, hits)| {
                let mut proposal = self.candidate(
                    ImprovementCategory::Tool,
                    ImprovementTarget::ToolRouter,
                    RiskTier::Moderate,
                    format!("Tool `{tool}` failed {} times", hits.len()),
                    format!(
                        "Investigate the failure pattern for `{tool}` and add a bounded \
                         retry/adapter or route around it."
                    ),
                    &hits,
                )?;
                proposal.root_cause_hypothesis =
                    format!("`{tool}` is invoked in a state it cannot satisfy");
                proposal.success_metrics = vec![
                    SuccessMetric {
                        id: "tool_failure_count".into(),
                        description: format!("failures of `{tool}` should fall toward zero"),
                        baseline: Some(hits.len() as f64),
                        target: Some(0.0),
                        hard_constraint: false,
                    },
                    Self::task_success_guard(),
                ];
                Some(proposal)
            })
            .collect()
    }

    fn repeated_workflow_candidates(&self, events: &[HarnessEvent]) -> Vec<ImprovementProposal> {
        let mut by_objective: BTreeMap<String, Vec<&HarnessEvent>> = BTreeMap::new();
        for event in events
            .iter()
            .filter(|e| e.event_type == event_type::TASK_COMPLETED)
        {
            // Group on the redacted summary — same completed objective repeated.
            by_objective
                .entry(event.summary.clone())
                .or_default()
                .push(event);
        }
        by_objective
            .into_iter()
            .filter(|(_, hits)| hits.len() >= self.min_occurrences)
            .filter_map(|(summary, hits)| {
                let mut proposal = self.candidate(
                    ImprovementCategory::Skill,
                    ImprovementTarget::Skill,
                    RiskTier::Moderate,
                    format!("Repeated workflow detected ({} times)", hits.len()),
                    format!(
                        "Capture the repeated workflow as a reusable skill candidate: {summary}"
                    ),
                    &hits,
                )?;
                proposal.root_cause_hypothesis =
                    "a recurring workflow is being re-derived each time".into();
                Some(proposal)
            })
            .collect()
    }

    fn context_overflow_candidates(&self, events: &[HarnessEvent]) -> Vec<ImprovementProposal> {
        let hits: Vec<&HarnessEvent> = events
            .iter()
            .filter(|e| e.event_type == event_type::CONTEXT_OVERFLOW)
            .collect();
        if hits.len() < self.min_occurrences {
            return Vec::new();
        }
        let Some(mut proposal) = self.candidate(
            ImprovementCategory::Context,
            ImprovementTarget::TokenBudgetPolicy,
            RiskTier::Low,
            format!("Context overflowed {} times", hits.len()),
            "Tune retrieval/compaction so the prompt stays within budget without dropping \
             required context."
                .to_string(),
            &hits,
        ) else {
            return Vec::new();
        };
        proposal.root_cause_hypothesis =
            "retrieval selects more context than the turn needs".into();
        vec![proposal]
    }

    /// Build a candidate with the common RSI provenance and evidence wiring.
    fn candidate(
        &self,
        category: ImprovementCategory,
        target: ImprovementTarget,
        risk_tier: RiskTier,
        problem: String,
        proposed_change: String,
        evidence_events: &[&HarnessEvent],
    ) -> Option<ImprovementProposal> {
        let mut proposal = ImprovementProposal::new(category, problem, proposed_change).ok()?;
        proposal.target = target;
        proposal.risk_tier = risk_tier;
        proposal.created_by = "improvement_planner".into();
        proposal.status = ImprovementStatus::Observed;
        proposal.evidence = evidence_events
            .iter()
            .map(|event| EvidenceReference {
                criterion: "recurrence".into(),
                summary: event.summary.clone(),
                source_ref: event.id.clone(),
                passed: false,
                observed_at: event.timestamp.clone(),
            })
            .collect();
        Some(proposal)
    }

    /// A hard constraint every candidate carries: whatever else improves, task
    /// success must not regress. WARP treats this as a veto, never a soft target.
    fn task_success_guard() -> SuccessMetric {
        SuccessMetric {
            id: "task_success_must_not_decrease".into(),
            description: "overall task success rate must not fall".into(),
            baseline: None,
            target: None,
            hard_constraint: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::store::Store;
    use serde_json::Value;

    fn tool_failure(tool: &str) -> HarnessEvent {
        let mut e = HarnessEvent::new(event_type::TOOL_FAILURE, format!("`{tool}` failed"));
        e.metadata
            .insert("tool".into(), Value::from(tool.to_string()));
        e
    }

    fn completed(summary: &str) -> HarnessEvent {
        HarnessEvent::new(event_type::TASK_COMPLETED, summary.to_string())
    }

    #[test]
    fn single_failure_is_not_enough_for_a_candidate() {
        let planner = ImprovementPlanner::default();
        let events = vec![tool_failure("fs.read_file")];
        assert!(planner.plan(&events).is_empty());
    }

    #[test]
    fn repeated_tool_failure_becomes_a_moderate_tool_candidate() {
        let planner = ImprovementPlanner::new(3);
        let events = vec![
            tool_failure("fs.read_file"),
            tool_failure("fs.read_file"),
            tool_failure("fs.read_file"),
        ];
        let candidates = planner.plan(&events);
        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        assert_eq!(c.target, ImprovementTarget::ToolRouter);
        assert_eq!(c.risk_tier, RiskTier::Moderate);
        assert_eq!(c.status, ImprovementStatus::Observed);
        assert_eq!(c.created_by, "improvement_planner");
        assert_eq!(c.evidence.len(), 3);
        // Always carries the non-regression veto.
        assert!(c
            .success_metrics
            .iter()
            .any(|m| m.id == "task_success_must_not_decrease" && m.hard_constraint));
    }

    #[test]
    fn repeated_workflow_becomes_a_skill_candidate_and_persists() {
        let dir = tempfile::tempdir().expect("dir");
        let store = Store::open(&dir.path().join("nexus.db")).expect("store");
        let repo = HarnessRepository::new(store);
        let planner = ImprovementPlanner::new(2);
        let events = vec![
            completed("completed: format the project"),
            completed("completed: format the project"),
        ];
        let ids = planner.plan_and_record(&repo, &events).expect("record");
        assert_eq!(ids.len(), 1);
        let saved = repo.improvement(&ids[0]).expect("load");
        assert_eq!(saved.target, ImprovementTarget::Skill);
        let observed = repo
            .improvement_proposals(Some(ImprovementStatus::Observed))
            .expect("list");
        assert_eq!(observed.len(), 1);
    }
}
