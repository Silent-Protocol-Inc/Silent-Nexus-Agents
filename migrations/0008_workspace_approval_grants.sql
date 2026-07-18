-- Revocable workspace-scoped approval grants.
-- Tokens are normalized by policy and never contain executable-only scopes
-- for commands whose structure is not explicitly understood.

CREATE TABLE workspace_approval_grants (
    workspace TEXT NOT NULL,
    grant_token TEXT NOT NULL,
    created_at TEXT NOT NULL,
    revoked_at TEXT,
    PRIMARY KEY(workspace, grant_token)
);

CREATE INDEX idx_workspace_approval_grants_active
    ON workspace_approval_grants(workspace, revoked_at, created_at);
