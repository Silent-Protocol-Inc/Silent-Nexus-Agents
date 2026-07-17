//! Subagent orchestration.
//!
//! The orchestrator spawns specialized subagents with narrowed permissions.
//! Concurrency is bounded and read-only research can fan out in parallel;
//! write-capable agents run sequentially to avoid conflicting edits. Every
//! spawn records *why* it exists and what concrete output is expected — no
//! uncontrolled swarms.

use crate::agents::AgentRole;
use crate::loop_engine::{AgentLoop, ApprovalHandler, LoopOutcome};
use crate::AgentRuntime;
use nexus_core::ids::SessionId;
use nexus_core::Result;
use std::sync::Arc;

/// A single delegation request.
#[derive(Debug, Clone)]
pub struct Delegation {
    pub role: AgentRole,
    pub objective: String,
    /// Why the orchestrator created this subagent.
    pub rationale: String,
    /// The concrete artifact/answer expected back.
    pub expected_output: String,
}

/// Result of a delegation.
#[derive(Debug, Clone)]
pub struct DelegationResult {
    pub role: AgentRole,
    pub objective: String,
    pub outcome: LoopOutcome,
}

pub struct Orchestrator {
    runtime: AgentRuntime,
    /// Maximum concurrent read-only subagents.
    max_concurrency: usize,
}

impl Orchestrator {
    pub fn new(runtime: AgentRuntime) -> Self {
        let configured = usize::try_from(runtime.limits.max_subagents).unwrap_or(usize::MAX);
        Self {
            runtime,
            max_concurrency: 3.min(configured.max(1)),
        }
    }

    pub fn with_concurrency(mut self, n: usize) -> Self {
        let configured = usize::try_from(self.runtime.limits.max_subagents).unwrap_or(usize::MAX);
        self.max_concurrency = n.max(1).min(configured.max(1));
        self
    }

    fn validate_delegations(&self, count: usize) -> Result<()> {
        validate_delegation_budget(
            count,
            self.runtime.limits.max_subagents,
            self.runtime.recursion_depth,
            self.runtime.limits.max_recursion_depth,
        )
    }

    fn child_runtime(&self) -> AgentRuntime {
        let mut runtime = self.runtime.clone();
        runtime.recursion_depth = runtime.recursion_depth.saturating_add(1);
        runtime
    }

    /// Run read-only delegations in parallel (bounded). Rejects any
    /// write-capable role to keep parallelism conflict-free.
    pub async fn fan_out_readonly(
        &self,
        session: &SessionId,
        delegations: Vec<Delegation>,
        approver: Arc<dyn ApprovalHandler>,
    ) -> Result<Vec<DelegationResult>> {
        self.validate_delegations(delegations.len())?;
        for d in &delegations {
            if d.role.can_write() {
                return Err(nexus_core::NexusError::Other(format!(
                    "role `{}` can write and may not run in a parallel read-only fan-out",
                    d.role.as_str()
                )));
            }
        }
        let mut results = Vec::new();
        for chunk in delegations.chunks(self.max_concurrency) {
            let mut handles = Vec::new();
            for d in chunk {
                let runtime = self.child_runtime();
                let approver = approver.clone();
                let session = session.clone();
                let d = d.clone();
                handles.push(tokio::spawn(async move {
                    let agent = AgentLoop::new(runtime, d.role);
                    let outcome = agent.run(&session, &d.objective, approver).await;
                    (d, outcome)
                }));
            }
            for handle in handles {
                let (d, outcome) = handle
                    .await
                    .map_err(|e| nexus_core::NexusError::other(format!("subagent join: {e}")))?;
                results.push(DelegationResult {
                    role: d.role,
                    objective: d.objective,
                    outcome: outcome?,
                });
            }
        }
        Ok(results)
    }

    /// Run delegations sequentially (required when any may write).
    pub async fn run_sequential(
        &self,
        session: &SessionId,
        delegations: Vec<Delegation>,
        approver: Arc<dyn ApprovalHandler>,
    ) -> Result<Vec<DelegationResult>> {
        self.validate_delegations(delegations.len())?;
        let mut results = Vec::new();
        for d in delegations {
            tracing::info!(
                role = d.role.as_str(),
                rationale = %d.rationale,
                expected = %d.expected_output,
                "spawning subagent"
            );
            let agent = AgentLoop::new(self.child_runtime(), d.role);
            let outcome = agent.run(session, &d.objective, approver.clone()).await?;
            results.push(DelegationResult {
                role: d.role,
                objective: d.objective,
                outcome,
            });
        }
        Ok(results)
    }
}

fn validate_delegation_budget(
    count: usize,
    max_subagents: u32,
    recursion_depth: u8,
    max_recursion_depth: u8,
) -> Result<()> {
    let max_subagents = usize::try_from(max_subagents).unwrap_or(usize::MAX);
    if count > max_subagents {
        return Err(nexus_core::NexusError::BudgetExhausted(format!(
            "subagent request {count} exceeds the configured limit {max_subagents}"
        )));
    }
    if count > 0 && u32::from(recursion_depth) >= u32::from(max_recursion_depth) {
        return Err(nexus_core::NexusError::BudgetExhausted(format!(
            "delegation depth {recursion_depth} reached the configured recursion limit {max_recursion_depth}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_delegation_budget;

    #[test]
    fn delegation_count_and_recursion_are_hard_limits() {
        assert!(validate_delegation_budget(2, 2, 0, 2).is_ok());
        assert!(validate_delegation_budget(3, 2, 0, 2).is_err());
        assert!(validate_delegation_budget(1, 2, 2, 2).is_err());
        assert!(validate_delegation_budget(0, 0, 0, 0).is_ok());
    }
}
