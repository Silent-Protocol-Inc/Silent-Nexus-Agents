-- Silent NEXUS adaptive harness persistence foundation.
-- Append-only migration. Migrations 0001-0005 are intentionally untouched.

CREATE TABLE harness_active_contexts (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    session_id TEXT,
    profile_id TEXT,
    persona_id TEXT,
    persona_version INTEGER,
    agent_id TEXT,
    goal_id TEXT,
    plan_id TEXT,
    plan_version INTEGER,
    task_id TEXT,
    provider_id TEXT,
    model_id TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_harness_context_workspace_session
    ON harness_active_contexts(workspace_id, COALESCE(session_id, ''));
CREATE INDEX idx_harness_context_profile
    ON harness_active_contexts(profile_id, status);

CREATE TABLE harness_profiles (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_seen_at TEXT
);
CREATE INDEX idx_harness_profiles_name
    ON harness_profiles(normalized_name, status);

CREATE TABLE harness_profile_facts (
    id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL REFERENCES harness_profiles(id) ON DELETE RESTRICT,
    fact_key TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'candidate',
    source_type TEXT NOT NULL,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    sensitivity TEXT NOT NULL DEFAULT 'normal',
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    expires_at TEXT
);
CREATE INDEX idx_harness_profile_facts_lookup
    ON harness_profile_facts(profile_id, fact_key, status);

CREATE TABLE harness_identity_conflicts (
    id TEXT PRIMARY KEY,
    active_profile_id TEXT REFERENCES harness_profiles(id) ON DELETE RESTRICT,
    candidate_profile_id TEXT REFERENCES harness_profiles(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'pending',
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    resolved_at TEXT
);
CREATE INDEX idx_harness_identity_conflicts_pending
    ON harness_identity_conflicts(active_profile_id, status, created_at);

CREATE TABLE harness_memories (
    id TEXT PRIMARY KEY,
    memory_type TEXT NOT NULL,
    scope_fingerprint TEXT NOT NULL,
    scope_kind TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    profile_id TEXT,
    workspace_id TEXT,
    project_id TEXT,
    session_id TEXT,
    goal_id TEXT,
    plan_id TEXT,
    task_id TEXT,
    agent_id TEXT,
    status TEXT NOT NULL DEFAULT 'candidate',
    source_type TEXT NOT NULL,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    importance REAL NOT NULL CHECK (importance >= 0.0 AND importance <= 1.0),
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_accessed_at TEXT,
    expires_at TEXT
);
CREATE INDEX idx_harness_memory_scope
    ON harness_memories(scope_fingerprint, status, memory_type, updated_at);
CREATE INDEX idx_harness_memory_profile
    ON harness_memories(profile_id, status, updated_at);
CREATE UNIQUE INDEX idx_harness_memory_dedup
    ON harness_memories(scope_fingerprint, memory_type, content_hash)
    WHERE status NOT IN ('deleted', 'rejected');

CREATE TABLE harness_persona_versions (
    persona_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    name TEXT NOT NULL,
    scope_kind TEXT NOT NULL,
    scope_key TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'inactive',
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(persona_id, version)
);
CREATE INDEX idx_harness_persona_scope
    ON harness_persona_versions(scope_kind, scope_key, status, updated_at);

CREATE TABLE harness_persona_assignments (
    id TEXT PRIMARY KEY,
    persona_id TEXT NOT NULL,
    persona_version INTEGER NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    precedence INTEGER NOT NULL DEFAULT 0,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(persona_id, persona_version)
        REFERENCES harness_persona_versions(persona_id, version) ON DELETE RESTRICT
);
CREATE INDEX idx_harness_persona_assignment_target
    ON harness_persona_assignments(target_kind, target_id, status, precedence);

CREATE TABLE harness_agent_definitions (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_harness_agent_name_active
    ON harness_agent_definitions(name) WHERE status = 'active';

CREATE TABLE harness_goals (
    id TEXT PRIMARY KEY,
    -- Profiles may live in the process-wide global store while goals remain
    -- workspace-local. This is an indexed cross-store reference, validated by
    -- the application control plane rather than an invalid SQLite foreign key.
    owner_profile_id TEXT,
    workspace_id TEXT NOT NULL,
    project_id TEXT,
    status TEXT NOT NULL DEFAULT 'draft',
    priority INTEGER NOT NULL DEFAULT 0,
    active_plan_id TEXT,
    active_plan_version INTEGER,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_harness_goal_workspace
    ON harness_goals(workspace_id, status, priority, updated_at);
CREATE INDEX idx_harness_goal_profile
    ON harness_goals(owner_profile_id, status, updated_at);

CREATE TABLE harness_plans (
    id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    goal_id TEXT NOT NULL REFERENCES harness_goals(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'draft',
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    approved_at TEXT,
    PRIMARY KEY(id, version)
);
CREATE INDEX idx_harness_plan_goal
    ON harness_plans(goal_id, status, version);

CREATE TABLE harness_tasks (
    id TEXT PRIMARY KEY,
    goal_id TEXT REFERENCES harness_goals(id) ON DELETE RESTRICT,
    plan_id TEXT,
    plan_version INTEGER,
    phase_id TEXT,
    parent_task_id TEXT REFERENCES harness_tasks(id) ON DELETE RESTRICT,
    assigned_agent_id TEXT,
    assigned_subagent_id TEXT,
    status TEXT NOT NULL DEFAULT 'draft',
    priority INTEGER NOT NULL DEFAULT 0,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(plan_id, plan_version)
        REFERENCES harness_plans(id, version) ON DELETE RESTRICT
);
CREATE INDEX idx_harness_task_plan
    ON harness_tasks(plan_id, plan_version, status, priority, updated_at);
CREATE INDEX idx_harness_task_goal
    ON harness_tasks(goal_id, status, priority, updated_at);

CREATE TABLE harness_task_edges (
    plan_id TEXT NOT NULL,
    plan_version INTEGER NOT NULL,
    from_task_id TEXT NOT NULL REFERENCES harness_tasks(id) ON DELETE RESTRICT,
    to_task_id TEXT NOT NULL REFERENCES harness_tasks(id) ON DELETE RESTRICT,
    relation TEXT NOT NULL DEFAULT 'blocks',
    created_at TEXT NOT NULL,
    PRIMARY KEY(plan_id, plan_version, from_task_id, to_task_id, relation),
    FOREIGN KEY(plan_id, plan_version)
        REFERENCES harness_plans(id, version) ON DELETE RESTRICT,
    CHECK (from_task_id <> to_task_id)
);
CREATE INDEX idx_harness_task_edges_to
    ON harness_task_edges(plan_id, plan_version, to_task_id);

CREATE TABLE harness_subagent_specs (
    id TEXT PRIMARY KEY,
    parent_agent_id TEXT NOT NULL,
    parent_goal_id TEXT,
    parent_plan_id TEXT,
    parent_plan_version INTEGER,
    parent_task_id TEXT,
    status TEXT NOT NULL DEFAULT 'draft',
    assignment_fingerprint TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_harness_subagent_active_assignment
    ON harness_subagent_specs(parent_agent_id, assignment_fingerprint)
    WHERE status IN ('configured', 'queued', 'running', 'waiting', 'under_review');
CREATE INDEX idx_harness_subagent_parent
    ON harness_subagent_specs(parent_task_id, status, updated_at);

CREATE TABLE harness_loop_states (
    run_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    profile_id TEXT,
    goal_id TEXT,
    plan_id TEXT,
    plan_version INTEGER,
    task_id TEXT,
    agent_id TEXT,
    status TEXT NOT NULL,
    progress_fingerprint TEXT,
    no_progress_count INTEGER NOT NULL DEFAULT 0,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_harness_loop_session
    ON harness_loop_states(session_id, status, updated_at);
CREATE INDEX idx_harness_loop_task
    ON harness_loop_states(task_id, status, updated_at);

CREATE TABLE harness_checkpoints (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    run_id TEXT,
    goal_id TEXT,
    plan_id TEXT,
    plan_version INTEGER,
    task_id TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    environment_fingerprint TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_harness_checkpoint_session
    ON harness_checkpoints(session_id, status, created_at);

CREATE TABLE harness_improvement_proposals (
    id TEXT PRIMARY KEY,
    category TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'draft',
    approval_required INTEGER NOT NULL DEFAULT 1,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    reviewed_at TEXT
);
CREATE INDEX idx_harness_improvement_status
    ON harness_improvement_proposals(status, category, updated_at);

CREATE TABLE harness_events (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    at TEXT NOT NULL,
    session_id TEXT,
    profile_id TEXT,
    goal_id TEXT,
    plan_id TEXT,
    task_id TEXT,
    agent_id TEXT,
    subagent_id TEXT,
    run_id TEXT,
    sensitivity TEXT NOT NULL DEFAULT 'normal',
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json))
);
CREATE INDEX idx_harness_event_session
    ON harness_events(session_id, at);
CREATE INDEX idx_harness_event_type
    ON harness_events(event_type, at);
CREATE INDEX idx_harness_event_work
    ON harness_events(goal_id, plan_id, task_id, at);

CREATE TABLE harness_provider_privacy_grants (
    id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL,
    scope_fingerprint TEXT NOT NULL,
    scope_kind TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    revoked_at TEXT
);
CREATE UNIQUE INDEX idx_harness_provider_privacy_scope
    ON harness_provider_privacy_grants(provider_id, scope_fingerprint)
    WHERE status='active';

CREATE TABLE harness_model_assignments (
    id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    fallback_priority INTEGER NOT NULL DEFAULT 0,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_harness_model_assignment_target
    ON harness_model_assignments(target_kind, target_id, status, fallback_priority);

CREATE TABLE harness_task_attempts (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES harness_tasks(id) ON DELETE RESTRICT,
    attempt_number INTEGER NOT NULL CHECK (attempt_number > 0),
    status TEXT NOT NULL DEFAULT 'running',
    provider_id TEXT,
    model_id TEXT,
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    finished_at TEXT,
    UNIQUE(task_id, attempt_number)
);
CREATE INDEX idx_harness_task_attempt_status
    ON harness_task_attempts(task_id, status, updated_at);

CREATE TABLE harness_resource_claims (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES harness_tasks(id) ON DELETE RESTRICT,
    resource_kind TEXT NOT NULL,
    resource_key TEXT NOT NULL,
    access_mode TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    expires_at TEXT
);
CREATE INDEX idx_harness_resource_claim_lookup
    ON harness_resource_claims(resource_kind, resource_key, status, access_mode, expires_at);
CREATE INDEX idx_harness_resource_claim_task
    ON harness_resource_claims(task_id, status, updated_at);

CREATE TABLE harness_approval_requests (
    id TEXT PRIMARY KEY,
    session_id TEXT,
    task_id TEXT,
    run_id TEXT,
    requesting_agent_id TEXT,
    provider_id TEXT,
    model_id TEXT,
    risk_class TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    grant_scope TEXT NOT NULL DEFAULT 'once',
    schema_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    resolved_at TEXT
);
CREATE INDEX idx_harness_approval_pending
    ON harness_approval_requests(session_id, status, created_at);
CREATE INDEX idx_harness_approval_task
    ON harness_approval_requests(task_id, status, created_at);
