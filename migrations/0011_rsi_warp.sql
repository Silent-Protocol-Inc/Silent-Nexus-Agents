-- Governed RSI + WARP storage.
--
-- The candidate registry (harness_improvement_proposals) and the observation log
-- (harness_events) already exist from 0006 and are JSON-backed, so the richer
-- typed fields added to ImprovementProposal/HarnessEvent in schema v2 need no
-- column changes here — serde defaults carry old rows forward.
--
-- This migration adds the tables WARP needs that did not exist before: outcome
-- records, experiments, evaluator verdicts, metric baselines, promotions,
-- rollbacks, and policy snapshots. Every table is workspace-scoped for isolation,
-- keeps a validated JSON payload for the full structure, and carries its own
-- schema_version. Legacy rsi_proposals (0002) is retained read-only and is
-- superseded by harness_improvement_proposals; it is not dropped so existing
-- rows and `snx profile` history remain available during the deprecation window.

-- Multi-dimensional outcome per completed task. Quality is kept as separate
-- dimensions on purpose: no single averaged score can hide a safety regression.
CREATE TABLE rsi_outcomes (
    id TEXT PRIMARY KEY,
    workspace_key TEXT NOT NULL,
    session_id TEXT,
    task_id TEXT,
    completion_status TEXT NOT NULL,
    final_score REAL,
    confidence REAL,
    created_at TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json))
);
CREATE INDEX idx_rsi_outcomes_workspace ON rsi_outcomes(workspace_key, created_at);
CREATE INDEX idx_rsi_outcomes_task ON rsi_outcomes(task_id, created_at);

-- A WARP validation run for one candidate.
CREATE TABLE warp_experiments (
    id TEXT PRIMARY KEY,
    workspace_key TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    stage TEXT NOT NULL,
    status TEXT NOT NULL,
    isolation_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json))
);
CREATE INDEX idx_warp_experiments_candidate ON warp_experiments(candidate_id, updated_at);
CREATE INDEX idx_warp_experiments_status ON warp_experiments(workspace_key, status, updated_at);

-- One independent evaluator's verdict for an experiment. Author reasoning is
-- never stored here — evaluators receive requirements + evidence only.
CREATE TABLE warp_evaluations (
    id TEXT PRIMARY KEY,
    experiment_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    evaluator_role TEXT NOT NULL,
    verdict TEXT NOT NULL,
    confidence REAL,
    provider TEXT,
    model TEXT,
    created_at TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json))
);
CREATE INDEX idx_warp_evaluations_experiment ON warp_evaluations(experiment_id, created_at);

-- Metric samples for a target at a given version, used for baseline vs candidate
-- distribution comparison (multiple samples for nondeterministic models).
CREATE TABLE warp_baselines (
    id TEXT PRIMARY KEY,
    workspace_key TEXT NOT NULL,
    target TEXT NOT NULL,
    version TEXT NOT NULL,
    metric TEXT NOT NULL,
    value REAL NOT NULL,
    sample_index INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_warp_baselines_lookup ON warp_baselines(workspace_key, target, metric, version);

-- A promoted change: enough state to attribute and reverse it.
CREATE TABLE rsi_promotions (
    id TEXT PRIMARY KEY,
    workspace_key TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    version TEXT NOT NULL,
    parent_version TEXT,
    promoted_commit TEXT,
    promoted_at TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json))
);
CREATE INDEX idx_rsi_promotions_candidate ON rsi_promotions(candidate_id, promoted_at);
CREATE INDEX idx_rsi_promotions_workspace ON rsi_promotions(workspace_key, promoted_at);

-- A rollback of a promotion, with its trigger. Append-only audit companion.
CREATE TABLE rsi_rollbacks (
    id TEXT PRIMARY KEY,
    workspace_key TEXT NOT NULL,
    promotion_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    trigger TEXT NOT NULL,
    rolled_back_at TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json))
);
CREATE INDEX idx_rsi_rollbacks_promotion ON rsi_rollbacks(promotion_id, rolled_back_at);

-- Snapshots of the effective governance + promotion policy at decision time, so
-- an audit can reconstruct which rules were in force for any promotion.
CREATE TABLE rsi_policies (
    id TEXT PRIMARY KEY,
    workspace_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json))
);
CREATE INDEX idx_rsi_policies_kind ON rsi_policies(workspace_key, kind, created_at);
