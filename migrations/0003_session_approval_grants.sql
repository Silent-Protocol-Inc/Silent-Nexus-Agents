-- Exact, session-scoped approval grants.
-- Only normalized non-destructive command tokens are stored by the agent loop.

CREATE TABLE session_approval_grants (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    grant_token TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(session_id, grant_token)
);
