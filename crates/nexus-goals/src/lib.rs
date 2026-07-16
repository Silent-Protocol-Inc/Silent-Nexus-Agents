//! nexus-goals: the persistent goal engine.
//!
//! A goal is durable workflow state, not a prompt alias. Every field —
//! acceptance criteria, constraints, allowed/prohibited paths, budgets, plan,
//! checkpoints, evidence, blockers, status — lives in SQLite and survives
//! restarts. Status transitions are validated and journaled to `goal_events`,
//! and a goal is only `completed` when every acceptance criterion has recorded
//! evidence.

use nexus_core::ids::{GoalId, SessionId, StepId};
use nexus_core::store::Store;
use nexus_core::{NexusError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Draft,
    Planned,
    Running,
    WaitingApproval,
    Blocked,
    Paused,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

impl GoalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalStatus::Draft => "draft",
            GoalStatus::Planned => "planned",
            GoalStatus::Running => "running",
            GoalStatus::WaitingApproval => "waiting_approval",
            GoalStatus::Blocked => "blocked",
            GoalStatus::Paused => "paused",
            GoalStatus::Verifying => "verifying",
            GoalStatus::Completed => "completed",
            GoalStatus::Failed => "failed",
            GoalStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "draft" => GoalStatus::Draft,
            "planned" => GoalStatus::Planned,
            "running" => GoalStatus::Running,
            "waiting_approval" => GoalStatus::WaitingApproval,
            "blocked" => GoalStatus::Blocked,
            "paused" => GoalStatus::Paused,
            "verifying" => GoalStatus::Verifying,
            "completed" => GoalStatus::Completed,
            "failed" => GoalStatus::Failed,
            "cancelled" => GoalStatus::Cancelled,
            _ => return None,
        })
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            GoalStatus::Completed | GoalStatus::Failed | GoalStatus::Cancelled
        )
    }

    /// Whether a transition to `next` is allowed.
    pub fn can_transition_to(&self, next: GoalStatus) -> bool {
        use GoalStatus::*;
        if *self == next {
            return true;
        }
        match self {
            Draft => matches!(next, Planned | Cancelled | Failed),
            Planned => matches!(next, Running | Paused | Cancelled | Failed),
            Running => matches!(
                next,
                WaitingApproval | Blocked | Paused | Verifying | Failed | Cancelled
            ),
            WaitingApproval => matches!(next, Running | Blocked | Paused | Cancelled | Failed),
            Blocked => matches!(next, Running | Paused | Cancelled | Failed),
            Paused => matches!(next, Running | Planned | Cancelled | Failed),
            Verifying => matches!(next, Completed | Running | Failed | Blocked),
            Completed | Failed | Cancelled => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceItem {
    /// Which acceptance criterion (index) this supports.
    pub criterion_index: usize,
    pub description: String,
    /// e.g. "repo.check", "fs.read_file", "terminal.run".
    pub source_tool: String,
    /// Reference to an artifact holding the full proof, if large.
    pub artifact_id: Option<String>,
    pub passed: bool,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalStep {
    pub id: StepId,
    pub seq: i64,
    pub description: String,
    pub status: String,
    pub evidence: Vec<EvidenceItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: GoalId,
    pub title: String,
    pub objective: String,
    pub acceptance_criteria: Vec<String>,
    pub constraints: Vec<String>,
    pub allowed_paths: Vec<String>,
    pub prohibited_paths: Vec<String>,
    pub model_policy: String,
    pub sandbox_policy: String,
    pub step_budget: i64,
    pub steps_used: i64,
    pub token_budget: i64,
    pub tokens_used: i64,
    pub runtime_budget_min: i64,
    pub runtime_used_ms: i64,
    pub status: GoalStatus,
    pub blockers: Vec<String>,
    pub session_id: Option<SessionId>,
    pub workspace: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Parameters for creating a goal.
#[derive(Debug, Clone)]
pub struct NewGoal {
    pub title: String,
    pub objective: String,
    pub acceptance_criteria: Vec<String>,
    pub constraints: Vec<String>,
    pub allowed_paths: Vec<String>,
    pub prohibited_paths: Vec<String>,
    pub step_budget: i64,
    pub token_budget: i64,
    pub runtime_budget_min: i64,
    pub workspace: String,
}

pub struct GoalStore {
    store: Store,
}

impl GoalStore {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn create(&self, g: NewGoal) -> Result<GoalId> {
        if g.objective.trim().is_empty() {
            return Err(NexusError::Other("goal objective is empty".into()));
        }
        let id = GoalId::generate();
        let now = nexus_core::now_rfc3339();
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO goals
                 (id, title, objective, acceptance_criteria, constraints_json, allowed_paths, prohibited_paths,
                  step_budget, token_budget, runtime_budget_min, status, workspace, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)",
                rusqlite::params![
                    id.as_str(),
                    g.title,
                    g.objective,
                    serde_json::to_string(&g.acceptance_criteria)?,
                    serde_json::to_string(&g.constraints)?,
                    serde_json::to_string(&g.allowed_paths)?,
                    serde_json::to_string(&g.prohibited_paths)?,
                    g.step_budget,
                    g.token_budget,
                    g.runtime_budget_min,
                    GoalStatus::Draft.as_str(),
                    g.workspace,
                    now,
                ],
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn get(&self, id: &str) -> Result<Goal> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id, title, objective, acceptance_criteria, constraints_json, allowed_paths,
                        prohibited_paths, model_policy, sandbox_policy, step_budget, steps_used,
                        token_budget, tokens_used, runtime_budget_min, runtime_used_ms,
                        status, blockers, session_id, workspace,
                        created_at, updated_at
                 FROM goals WHERE id = ?1",
                [id],
                row_to_goal,
            )
            .map_err(|_| NexusError::NotFound(format!("goal `{id}`")))
        })
    }

    pub fn list(&self, workspace: Option<&str>) -> Result<Vec<Goal>> {
        self.store.with(|conn| {
            let (sql, param): (&str, Vec<String>) = match workspace {
                Some(ws) => (
                    "SELECT id, title, objective, acceptance_criteria, constraints_json, allowed_paths,
                            prohibited_paths, model_policy, sandbox_policy, step_budget, steps_used,
                            token_budget, tokens_used, runtime_budget_min, runtime_used_ms,
                            status, blockers, session_id, workspace,
                            created_at, updated_at
                     FROM goals WHERE workspace = ?1 ORDER BY created_at DESC",
                    vec![ws.to_string()],
                ),
                None => (
                    "SELECT id, title, objective, acceptance_criteria, constraints_json, allowed_paths,
                            prohibited_paths, model_policy, sandbox_policy, step_budget, steps_used,
                            token_budget, tokens_used, runtime_budget_min, runtime_used_ms,
                            status, blockers, session_id, workspace,
                            created_at, updated_at
                     FROM goals ORDER BY created_at DESC",
                    vec![],
                ),
            };
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(param), row_to_goal)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
    }

    /// Replace the plan (steps). Used by `snx goal plan`.
    pub fn set_plan(&self, goal_id: &str, steps: &[String]) -> Result<()> {
        self.store.with(|conn| {
            conn.execute("DELETE FROM goal_steps WHERE goal_id = ?1", [goal_id])?;
            for (i, desc) in steps.iter().enumerate() {
                conn.execute(
                    "INSERT INTO goal_steps (id, goal_id, seq, description, status)
                     VALUES (?1,?2,?3,?4,'pending')",
                    rusqlite::params![StepId::generate().as_str(), goal_id, i as i64, desc],
                )?;
            }
            Ok(())
        })?;
        self.transition(goal_id, GoalStatus::Planned, "plan created")?;
        Ok(())
    }

    pub fn steps(&self, goal_id: &str) -> Result<Vec<GoalStep>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, seq, description, status, evidence FROM goal_steps
                 WHERE goal_id = ?1 ORDER BY seq",
            )?;
            let rows = stmt.query_map([goal_id], |row| {
                let evidence_json: String = row.get(4)?;
                Ok(GoalStep {
                    id: StepId::from(row.get::<_, String>(0)?),
                    seq: row.get(1)?,
                    description: row.get(2)?,
                    status: row.get(3)?,
                    evidence: serde_json::from_str(&evidence_json).unwrap_or_default(),
                })
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
    }

    pub fn set_step_status(&self, step_id: &str, status: &str) -> Result<()> {
        let ts = nexus_core::now_rfc3339();
        self.store.with(|conn| {
            let col = match status {
                "running" => "started_at",
                "done" | "failed" | "skipped" => "finished_at",
                _ => "started_at",
            };
            conn.execute(
                &format!("UPDATE goal_steps SET status = ?1, {col} = ?2 WHERE id = ?3"),
                rusqlite::params![status, ts, step_id],
            )?;
            Ok(())
        })
    }

    /// Attach evidence to a step.
    pub fn add_evidence(&self, step_id: &str, item: EvidenceItem) -> Result<()> {
        self.store.with(|conn| {
            let current: String = conn
                .query_row(
                    "SELECT evidence FROM goal_steps WHERE id = ?1",
                    [step_id],
                    |r| r.get(0),
                )
                .map_err(|_| NexusError::NotFound(format!("step `{step_id}`")))?;
            let mut list: Vec<EvidenceItem> = serde_json::from_str(&current).unwrap_or_default();
            list.push(item);
            conn.execute(
                "UPDATE goal_steps SET evidence = ?1 WHERE id = ?2",
                rusqlite::params![serde_json::to_string(&list)?, step_id],
            )?;
            Ok(())
        })
    }

    /// Record consumption against budgets. Returns an error when a budget is
    /// exhausted so the agent loop can stop.
    pub fn consume_budget(
        &self,
        goal_id: &str,
        steps: i64,
        runtime_ms: i64,
        tokens: i64,
    ) -> Result<()> {
        let goal = self.get(goal_id)?;
        let new_steps = goal.steps_used + steps;
        let new_runtime = goal.runtime_used_ms + runtime_ms;
        let new_tokens = goal.tokens_used + tokens;
        self.store.with(|conn| {
            conn.execute(
                "UPDATE goals SET steps_used = ?1, runtime_used_ms = ?2, tokens_used = ?3,
                                  updated_at = ?4 WHERE id = ?5",
                rusqlite::params![
                    new_steps,
                    new_runtime,
                    new_tokens,
                    nexus_core::now_rfc3339(),
                    goal_id
                ],
            )?;
            Ok(())
        })?;
        if goal.step_budget > 0 && new_steps > goal.step_budget {
            return Err(NexusError::BudgetExhausted(format!(
                "goal step budget of {} exceeded",
                goal.step_budget
            )));
        }
        if goal.runtime_budget_min > 0 && new_runtime > goal.runtime_budget_min * 60_000 {
            return Err(NexusError::BudgetExhausted(format!(
                "goal runtime budget of {} min exceeded",
                goal.runtime_budget_min
            )));
        }
        if goal.token_budget > 0 && new_tokens > goal.token_budget {
            return Err(NexusError::BudgetExhausted(format!(
                "goal token budget of {} exceeded",
                goal.token_budget
            )));
        }
        Ok(())
    }

    pub fn set_blockers(&self, goal_id: &str, blockers: &[String]) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "UPDATE goals SET blockers = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![
                    serde_json::to_string(blockers)?,
                    nexus_core::now_rfc3339(),
                    goal_id
                ],
            )?;
            Ok(())
        })
    }

    pub fn attach_session(&self, goal_id: &str, session: &SessionId) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "UPDATE goals SET session_id = ?1 WHERE id = ?2",
                rusqlite::params![session.as_str(), goal_id],
            )?;
            Ok(())
        })
    }

    /// Transition a goal to a new status, validating the edge and journaling
    /// it. Guards the completion invariant.
    pub fn transition(&self, goal_id: &str, next: GoalStatus, reason: &str) -> Result<()> {
        let goal = self.get(goal_id)?;
        if !goal.status.can_transition_to(next) {
            return Err(NexusError::Other(format!(
                "invalid goal transition {} → {}",
                goal.status.as_str(),
                next.as_str()
            )));
        }
        if next == GoalStatus::Completed {
            let verification = self.verify(goal_id)?;
            if !verification.all_satisfied {
                return Err(NexusError::Other(format!(
                    "cannot complete goal: {} of {} acceptance criteria lack passing evidence",
                    verification.unsatisfied.len(),
                    goal.acceptance_criteria.len()
                )));
            }
        }
        let now = nexus_core::now_rfc3339();
        self.store.with(|conn| {
            conn.execute(
                "UPDATE goals SET status = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![next.as_str(), now, goal_id],
            )?;
            conn.execute(
                "INSERT INTO goal_events (goal_id, at, from_status, to_status, reason)
                 VALUES (?1,?2,?3,?4,?5)",
                rusqlite::params![goal_id, now, goal.status.as_str(), next.as_str(), reason],
            )?;
            Ok(())
        })?;
        tracing::info!(
            goal = goal_id,
            from = goal.status.as_str(),
            to = next.as_str(),
            "goal transition"
        );
        Ok(())
    }

    /// Verify acceptance criteria against recorded evidence.
    pub fn verify(&self, goal_id: &str) -> Result<Verification> {
        let goal = self.get(goal_id)?;
        let steps = self.steps(goal_id)?;
        let mut satisfied = vec![false; goal.acceptance_criteria.len()];
        for step in &steps {
            for ev in &step.evidence {
                if ev.passed && ev.criterion_index < satisfied.len() {
                    satisfied[ev.criterion_index] = true;
                }
            }
        }
        let unsatisfied: Vec<(usize, String)> = goal
            .acceptance_criteria
            .iter()
            .enumerate()
            .filter(|(i, _)| !satisfied[*i])
            .map(|(i, c)| (i, c.clone()))
            .collect();
        Ok(Verification {
            all_satisfied: unsatisfied.is_empty() && !goal.acceptance_criteria.is_empty(),
            satisfied_count: satisfied.iter().filter(|b| **b).count(),
            total: goal.acceptance_criteria.len(),
            unsatisfied,
        })
    }

    pub fn history(&self, goal_id: &str) -> Result<Vec<(String, String, String, String)>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT at, from_status, to_status, reason FROM goal_events
                 WHERE goal_id = ?1 ORDER BY id",
            )?;
            let rows = stmt.query_map([goal_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
    }

    /// Find goals that were mid-flight when the process stopped (running,
    /// verifying, waiting_approval) so `snx goal resume` / startup recovery can
    /// re-attach to them.
    pub fn recoverable(&self, workspace: &str) -> Result<Vec<Goal>> {
        Ok(self
            .list(Some(workspace))?
            .into_iter()
            .filter(|g| {
                matches!(
                    g.status,
                    GoalStatus::Running | GoalStatus::Verifying | GoalStatus::WaitingApproval
                )
            })
            .collect())
    }

    /// Export a goal and its plan/evidence as JSON.
    pub fn export(&self, goal_id: &str) -> Result<String> {
        let goal = self.get(goal_id)?;
        let steps = self.steps(goal_id)?;
        let history = self.history(goal_id)?;
        let verification = self.verify(goal_id)?;
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "goal": goal,
            "steps": steps,
            "history": history,
            "verification": {
                "all_satisfied": verification.all_satisfied,
                "satisfied": verification.satisfied_count,
                "total": verification.total,
                "unsatisfied": verification.unsatisfied,
            }
        }))?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    pub all_satisfied: bool,
    pub satisfied_count: usize,
    pub total: usize,
    pub unsatisfied: Vec<(usize, String)>,
}

fn row_to_goal(row: &rusqlite::Row) -> rusqlite::Result<Goal> {
    let parse_vec = |s: String| -> Vec<String> { serde_json::from_str(&s).unwrap_or_default() };
    Ok(Goal {
        id: GoalId::from(row.get::<_, String>(0)?),
        title: row.get(1)?,
        objective: row.get(2)?,
        acceptance_criteria: parse_vec(row.get(3)?),
        constraints: parse_vec(row.get(4)?),
        allowed_paths: parse_vec(row.get(5)?),
        prohibited_paths: parse_vec(row.get(6)?),
        model_policy: row.get(7)?,
        sandbox_policy: row.get(8)?,
        step_budget: row.get(9)?,
        steps_used: row.get(10)?,
        token_budget: row.get(11)?,
        tokens_used: row.get(12)?,
        runtime_budget_min: row.get(13)?,
        runtime_used_ms: row.get(14)?,
        status: GoalStatus::parse(&row.get::<_, String>(15)?).unwrap_or(GoalStatus::Draft),
        blockers: parse_vec(row.get(16)?),
        session_id: row.get::<_, Option<String>>(17)?.map(SessionId::from),
        workspace: row.get(18)?,
        created_at: row.get(19)?,
        updated_at: row.get(20)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal_store() -> GoalStore {
        GoalStore::new(Store::open_in_memory().expect("store"))
    }

    fn sample() -> NewGoal {
        NewGoal {
            title: "Add slash command".into(),
            objective: "Implement the /status slash command".into(),
            acceptance_criteria: vec!["command parses".into(), "tests pass".into()],
            constraints: vec!["no new dependencies".into()],
            allowed_paths: vec!["src/".into()],
            prohibited_paths: vec![],
            step_budget: 50,
            token_budget: 0,
            runtime_budget_min: 30,
            workspace: "/ws".into(),
        }
    }

    #[test]
    fn create_plan_and_status_flow() {
        let gs = goal_store();
        let id = gs.create(sample()).expect("create");
        assert_eq!(gs.get(id.as_str()).expect("get").status, GoalStatus::Draft);
        gs.set_plan(id.as_str(), &["step one".into(), "step two".into()])
            .expect("plan");
        assert_eq!(
            gs.get(id.as_str()).expect("get").status,
            GoalStatus::Planned
        );
        assert_eq!(gs.steps(id.as_str()).expect("steps").len(), 2);
        gs.transition(id.as_str(), GoalStatus::Running, "start")
            .expect("run");
    }

    #[test]
    fn invalid_transition_rejected() {
        let gs = goal_store();
        let id = gs.create(sample()).expect("create");
        // draft → completed is not allowed.
        assert!(gs
            .transition(id.as_str(), GoalStatus::Completed, "x")
            .is_err());
    }

    #[test]
    fn completion_requires_evidence_for_all_criteria() {
        let gs = goal_store();
        let id = gs.create(sample()).expect("create");
        gs.set_plan(id.as_str(), &["do work".into()]).expect("plan");
        gs.transition(id.as_str(), GoalStatus::Running, "start")
            .expect("run");
        gs.transition(id.as_str(), GoalStatus::Verifying, "verify")
            .expect("verify");
        let steps = gs.steps(id.as_str()).expect("steps");
        let step_id = steps[0].id.as_str().to_string();
        // Only satisfy criterion 0.
        gs.add_evidence(
            &step_id,
            EvidenceItem {
                criterion_index: 0,
                description: "parser test green".into(),
                source_tool: "repo.check".into(),
                artifact_id: None,
                passed: true,
                recorded_at: nexus_core::now_rfc3339(),
            },
        )
        .expect("evidence");
        // Missing criterion 1 → cannot complete.
        assert!(gs
            .transition(id.as_str(), GoalStatus::Completed, "done")
            .is_err());
        // Satisfy criterion 1.
        gs.add_evidence(
            &step_id,
            EvidenceItem {
                criterion_index: 1,
                description: "all tests pass".into(),
                source_tool: "repo.check".into(),
                artifact_id: None,
                passed: true,
                recorded_at: nexus_core::now_rfc3339(),
            },
        )
        .expect("evidence");
        gs.transition(id.as_str(), GoalStatus::Completed, "done")
            .expect("complete");
        assert_eq!(
            gs.get(id.as_str()).expect("get").status,
            GoalStatus::Completed
        );
    }

    #[test]
    fn budget_exhaustion_is_detected() {
        let gs = goal_store();
        let mut s = sample();
        s.step_budget = 2;
        let id = gs.create(s).expect("create");
        gs.consume_budget(id.as_str(), 1, 0, 0).expect("ok");
        let err = gs
            .consume_budget(id.as_str(), 5, 0, 0)
            .expect_err("over budget");
        assert!(matches!(err, NexusError::BudgetExhausted(_)));
    }

    #[test]
    fn token_budget_is_accounted_and_enforced() {
        let gs = goal_store();
        let mut s = sample();
        s.token_budget = 100;
        let id = gs.create(s).expect("create");
        gs.consume_budget(id.as_str(), 1, 10, 60).expect("within");
        let goal = gs.get(id.as_str()).expect("goal");
        assert_eq!(goal.tokens_used, 60);
        let err = gs
            .consume_budget(id.as_str(), 0, 0, 41)
            .expect_err("token budget");
        assert!(matches!(err, NexusError::BudgetExhausted(_)));
    }

    #[test]
    fn recoverable_finds_in_flight_goals() {
        let gs = goal_store();
        let id = gs.create(sample()).expect("create");
        gs.set_plan(id.as_str(), &["x".into()]).expect("plan");
        gs.transition(id.as_str(), GoalStatus::Running, "start")
            .expect("run");
        let recoverable = gs.recoverable("/ws").expect("recover");
        assert_eq!(recoverable.len(), 1);
        assert_eq!(recoverable[0].id, id);
    }

    #[test]
    fn history_is_journaled() {
        let gs = goal_store();
        let id = gs.create(sample()).expect("create");
        gs.set_plan(id.as_str(), &["x".into()]).expect("plan");
        gs.transition(id.as_str(), GoalStatus::Running, "begin work")
            .expect("run");
        let hist = gs.history(id.as_str()).expect("history");
        assert!(hist.iter().any(|(_, _, to, _)| to == "running"));
        assert!(hist.iter().any(|(_, _, _, reason)| reason == "begin work"));
    }
}
