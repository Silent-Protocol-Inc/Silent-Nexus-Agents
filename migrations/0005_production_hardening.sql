-- Silent Nexus 1.0 production hardening.
-- Append-only migration. Migrations 0001-0004 are intentionally untouched.

CREATE TABLE migration_checksums (
    name TEXT PRIMARY KEY REFERENCES schema_migrations(name) ON DELETE CASCADE,
    sha256 TEXT NOT NULL,
    verified_at TEXT NOT NULL
);

CREATE VIRTUAL TABLE timeline_fts USING fts5(
    summary,
    payload,
    content='timeline_events',
    content_rowid='rowid'
);

CREATE TRIGGER timeline_fts_ai AFTER INSERT ON timeline_events BEGIN
    INSERT INTO timeline_fts(rowid, summary, payload)
    VALUES (new.rowid, new.summary, new.payload);
END;
CREATE TRIGGER timeline_fts_ad AFTER DELETE ON timeline_events BEGIN
    INSERT INTO timeline_fts(timeline_fts, rowid, summary, payload)
    VALUES ('delete', old.rowid, old.summary, old.payload);
END;
CREATE TRIGGER timeline_fts_au AFTER UPDATE ON timeline_events BEGIN
    INSERT INTO timeline_fts(timeline_fts, rowid, summary, payload)
    VALUES ('delete', old.rowid, old.summary, old.payload);
    INSERT INTO timeline_fts(rowid, summary, payload)
    VALUES (new.rowid, new.summary, new.payload);
END;

INSERT INTO timeline_fts(rowid, summary, payload)
SELECT rowid, summary, payload FROM timeline_events;

CREATE INDEX idx_timeline_session_status_sequence
    ON timeline_events(session_id, status, sequence);
CREATE INDEX idx_timeline_session_source_sequence
    ON timeline_events(session_id, source, sequence);
CREATE INDEX idx_background_tasks_session_status_updated
    ON background_tasks(session_id, status, updated_at);
CREATE INDEX idx_agent_runs_session_status_updated
    ON agent_runs(session_id, status, updated_at);
CREATE INDEX idx_plan_steps_plan_status_seq
    ON plan_steps(plan_id, plan_version, status, seq);
