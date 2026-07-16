-- NEXUS interactive-agent upgrade.
-- Append-only migration: personas/profile learning, review candidates,
-- durable usage, rollover links, RSI proposals, and connector imports.

ALTER TABLE sessions ADD COLUMN persona_id TEXT;
ALTER TABLE sessions ADD COLUMN profile_name TEXT NOT NULL DEFAULT 'default';

CREATE TABLE personas (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'global',       -- global | project
    workspace TEXT NOT NULL DEFAULT '',
    parent_id TEXT REFERENCES personas(id) ON DELETE SET NULL,
    description TEXT NOT NULL DEFAULT '',
    instructions TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(name, scope, workspace)
);

CREATE TABLE profile_traits (
    id TEXT PRIMARY KEY,
    profile_name TEXT NOT NULL DEFAULT 'default',
    trait_key TEXT NOT NULL,
    trait_value TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'workflow',
    explicit INTEGER NOT NULL DEFAULT 0,
    confidence REAL NOT NULL DEFAULT 0.5,
    evidence TEXT NOT NULL DEFAULT '',
    source_session TEXT,
    sensitivity TEXT NOT NULL DEFAULT 'normal',
    status TEXT NOT NULL DEFAULT 'pending',     -- pending | approved | rejected
    scope TEXT NOT NULL DEFAULT 'project',
    workspace TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_profile_traits_lookup
    ON profile_traits(profile_name, workspace, status);

CREATE TABLE memory_candidates (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    evidence TEXT NOT NULL DEFAULT '',
    confidence REAL NOT NULL DEFAULT 0.5,
    source_session TEXT,
    sensitivity TEXT NOT NULL DEFAULT 'normal',
    status TEXT NOT NULL DEFAULT 'pending',     -- pending | approved | rejected
    scope TEXT NOT NULL DEFAULT 'project',
    workspace TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    reviewed_at TEXT
);

CREATE TABLE session_usage (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    provider TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    tool_calls INTEGER NOT NULL DEFAULT 0,
    elapsed_ms INTEGER NOT NULL DEFAULT 0,
    started_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    exit_at TEXT
);

CREATE TABLE session_links (
    parent_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    child_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    relation TEXT NOT NULL DEFAULT 'rollover',
    created_at TEXT NOT NULL,
    PRIMARY KEY(parent_session_id, child_session_id, relation)
);

CREATE TABLE rsi_proposals (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,                         -- skill | tool | connector | config | source
    title TEXT NOT NULL,
    body TEXT NOT NULL,                         -- inspectable JSON/markdown; no executable payload
    risk TEXT NOT NULL DEFAULT 'review',
    source_session TEXT,
    status TEXT NOT NULL DEFAULT 'pending',     -- pending | approved | rejected
    created_at TEXT NOT NULL,
    reviewed_at TEXT
);

CREATE TABLE connector_imports (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,                         -- mcp | skill
    name TEXT NOT NULL,
    source TEXT NOT NULL,
    preview TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 0,
    trust TEXT NOT NULL DEFAULT 'untrusted',
    created_at TEXT NOT NULL,
    UNIQUE(kind, name, source)
);
