-- Background task dependency edges. A queued task is leasable only when every
-- dependency has completed; tasks with a failed or cancelled dependency are
-- moved to 'blocked' instead of running against stale assumptions.
-- Append-only: no existing tables or rows are modified.
CREATE TABLE background_task_dependencies (
    task_id TEXT NOT NULL REFERENCES background_tasks(id) ON DELETE CASCADE,
    depends_on_task_id TEXT NOT NULL REFERENCES background_tasks(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (task_id, depends_on_task_id),
    CHECK (task_id <> depends_on_task_id)
);
CREATE INDEX idx_background_task_deps_dep
    ON background_task_dependencies(depends_on_task_id);

-- Resource claims may now be held by plan tasks (harness_tasks) or background
-- worker tasks (background_tasks), so the hard foreign key to harness_tasks is
-- replaced by an application-level existence check in
-- HarnessRepository::claim_resource. SQLite cannot drop a foreign key in
-- place; rebuild the table (it had no production writers before this
-- migration, but existing rows are copied defensively).
CREATE TABLE harness_resource_claims_v2 (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
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
INSERT INTO harness_resource_claims_v2 SELECT * FROM harness_resource_claims;
DROP TABLE harness_resource_claims;
ALTER TABLE harness_resource_claims_v2 RENAME TO harness_resource_claims;
CREATE INDEX idx_harness_resource_claim_lookup
    ON harness_resource_claims(resource_kind, resource_key, status, access_mode, expires_at);
