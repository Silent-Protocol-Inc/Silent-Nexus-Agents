-- NEXUS 0.2 orchestration timeline.
-- Append-only migration. Migrations 0001-0003 are intentionally untouched.

CREATE TABLE timeline_events (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_id TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    span_id TEXT NOT NULL,
    parent_span_id TEXT,
    sequence INTEGER NOT NULL,
    at TEXT NOT NULL,
    event_type TEXT NOT NULL,
    phase TEXT NOT NULL,
    status TEXT NOT NULL,
    duration_ms INTEGER,
    summary TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}',          -- redacted TimelineKind JSON
    artifact_refs TEXT NOT NULL DEFAULT '[]',    -- redacted ArtifactReference JSON
    risk TEXT,
    source TEXT NOT NULL DEFAULT 'native',       -- native | legacy_projection | command
    UNIQUE(session_id, sequence)
);
CREATE INDEX idx_timeline_session_sequence
    ON timeline_events(session_id, sequence);
CREATE INDEX idx_timeline_trace
    ON timeline_events(trace_id, sequence);
CREATE INDEX idx_timeline_type
    ON timeline_events(session_id, event_type, sequence);

CREATE TABLE context_manifests (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_id TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    estimated INTEGER NOT NULL DEFAULT 1,
    provider_input_tokens INTEGER,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    reserved_output_tokens INTEGER NOT NULL DEFAULT 0,
    context_window INTEGER NOT NULL DEFAULT 0,
    categories_json TEXT NOT NULL DEFAULT '[]',
    omissions_json TEXT NOT NULL DEFAULT '[]',
    payload TEXT NOT NULL
);
CREATE INDEX idx_context_manifest_session
    ON context_manifests(session_id, created_at);

CREATE TABLE session_view_state (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    last_read_sequence INTEGER NOT NULL DEFAULT 0,
    selected_filter TEXT NOT NULL DEFAULT 'all',
    detail_level TEXT NOT NULL DEFAULT 'compact',
    collapsed_cards TEXT NOT NULL DEFAULT '[]',
    search_query TEXT,
    updated_at TEXT NOT NULL
);

CREATE TABLE background_tasks (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    plan_id TEXT,
    stage_id TEXT,
    title TEXT NOT NULL,
    objective TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    owner TEXT NOT NULL DEFAULT 'worker',
    writer INTEGER NOT NULL DEFAULT 0,
    branch TEXT,
    worktree TEXT,
    budget_json TEXT NOT NULL DEFAULT '{}',
    result_json TEXT,
    error TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_expires_at TEXT,
    heartbeat_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT
);
CREATE INDEX idx_background_tasks_status
    ON background_tasks(status, updated_at);
CREATE INDEX idx_background_tasks_session
    ON background_tasks(session_id, created_at);

CREATE TABLE agent_runs (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    parent_run_id TEXT REFERENCES agent_runs(id) ON DELETE SET NULL,
    task_id TEXT REFERENCES background_tasks(id) ON DELETE SET NULL,
    role TEXT NOT NULL,
    objective TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    depth INTEGER NOT NULL DEFAULT 0,
    model TEXT NOT NULL DEFAULT '',
    permission_mode TEXT NOT NULL DEFAULT '',
    budget_json TEXT NOT NULL DEFAULT '{}',
    unread_events INTEGER NOT NULL DEFAULT 0,
    result_json TEXT,
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT
);
CREATE INDEX idx_agent_runs_session
    ON agent_runs(session_id, created_at);
CREATE INDEX idx_agent_runs_parent
    ON agent_runs(parent_run_id, created_at);

CREATE TABLE plan_versions (
    id TEXT NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    title TEXT NOT NULL,
    objective TEXT NOT NULL,
    breakdown_kind TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'draft',
    scope_hash TEXT NOT NULL,
    body_json TEXT NOT NULL,
    created_by TEXT NOT NULL DEFAULT 'harness',
    created_at TEXT NOT NULL,
    PRIMARY KEY(id, version)
);
CREATE INDEX idx_plan_versions_session
    ON plan_versions(session_id, created_at);

CREATE TABLE plan_steps (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    plan_version INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    owner TEXT NOT NULL DEFAULT 'main',
    budget_json TEXT NOT NULL DEFAULT '{}',
    evidence_json TEXT NOT NULL DEFAULT '[]',
    changed_files_json TEXT NOT NULL DEFAULT '[]',
    validation_json TEXT NOT NULL DEFAULT '[]',
    next_action TEXT,
    started_at TEXT,
    finished_at TEXT,
    FOREIGN KEY(plan_id, plan_version)
        REFERENCES plan_versions(id, version) ON DELETE CASCADE,
    UNIQUE(plan_id, plan_version, seq)
);

CREATE TABLE plan_edges (
    plan_id TEXT NOT NULL,
    plan_version INTEGER NOT NULL,
    from_step_id TEXT NOT NULL REFERENCES plan_steps(id) ON DELETE CASCADE,
    to_step_id TEXT NOT NULL REFERENCES plan_steps(id) ON DELETE CASCADE,
    relation TEXT NOT NULL DEFAULT 'blocks',
    PRIMARY KEY(plan_id, plan_version, from_step_id, to_step_id, relation),
    FOREIGN KEY(plan_id, plan_version)
        REFERENCES plan_versions(id, version) ON DELETE CASCADE
);

CREATE TABLE plan_approvals (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    plan_version INTEGER NOT NULL,
    approved INTEGER,
    scope_diff TEXT NOT NULL DEFAULT '{}',
    requested_at TEXT NOT NULL,
    resolved_at TEXT,
    approver TEXT,
    FOREIGN KEY(plan_id, plan_version)
        REFERENCES plan_versions(id, version) ON DELETE CASCADE
);

CREATE TABLE session_interruptions (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_id TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    provider TEXT,
    model TEXT,
    message TEXT NOT NULL,
    reset_at TEXT,
    retryable INTEGER NOT NULL DEFAULT 0,
    checkpoint_artifact TEXT,
    child_session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    resolved_at TEXT
);
CREATE INDEX idx_session_interruptions_session
    ON session_interruptions(session_id, created_at);
