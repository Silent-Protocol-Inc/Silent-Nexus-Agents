-- Silent Nexus initial schema.
-- Applied by nexus-core::store; recorded in schema_migrations.

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT '',
    workspace TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    model TEXT NOT NULL DEFAULT '',
    agent TEXT NOT NULL DEFAULT '',
    summary TEXT NOT NULL DEFAULT '',
    pending_tasks TEXT NOT NULL DEFAULT '[]',   -- JSON array
    changed_files TEXT NOT NULL DEFAULT '[]',   -- JSON array
    current_goal TEXT,
    status TEXT NOT NULL DEFAULT 'active'       -- active | archived
);

CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn INTEGER NOT NULL,
    role TEXT NOT NULL,                          -- system | user | assistant | tool
    content TEXT NOT NULL,
    tool_call_id TEXT,
    tool_name TEXT,
    created_at TEXT NOT NULL,
    compacted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_messages_session ON messages(session_id, id);

CREATE TABLE tool_calls (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    arguments TEXT NOT NULL,                     -- JSON, redacted
    risk TEXT NOT NULL,
    decision TEXT NOT NULL,
    exit_status TEXT,                            -- ok | error | timeout | denied
    output_preview TEXT NOT NULL DEFAULT '',
    artifact_id TEXT,
    idempotency_key TEXT,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    duration_ms INTEGER
);
CREATE INDEX idx_tool_calls_session ON tool_calls(session_id);
CREATE UNIQUE INDEX idx_tool_calls_idem ON tool_calls(idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE TABLE approvals (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    summary TEXT NOT NULL,
    risk TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    resolved_at TEXT,
    approved INTEGER,                            -- NULL = pending
    edited_command TEXT,
    scope TEXT NOT NULL DEFAULT 'once'           -- once | session
);

CREATE TABLE goals (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    objective TEXT NOT NULL,
    acceptance_criteria TEXT NOT NULL DEFAULT '[]',  -- JSON array
    constraints_json TEXT NOT NULL DEFAULT '[]',     -- JSON array
    allowed_paths TEXT NOT NULL DEFAULT '[]',
    prohibited_paths TEXT NOT NULL DEFAULT '[]',
    model_policy TEXT NOT NULL DEFAULT '',
    tool_permissions TEXT NOT NULL DEFAULT '{}',     -- JSON
    sandbox_policy TEXT NOT NULL DEFAULT '',
    step_budget INTEGER NOT NULL DEFAULT 200,
    steps_used INTEGER NOT NULL DEFAULT 0,
    token_budget INTEGER NOT NULL DEFAULT 0,         -- 0 = unlimited
    tokens_used INTEGER NOT NULL DEFAULT 0,
    runtime_budget_min INTEGER NOT NULL DEFAULT 120,
    runtime_used_ms INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'draft',
    blockers TEXT NOT NULL DEFAULT '[]',             -- JSON array
    session_id TEXT,
    workspace TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE goal_steps (
    id TEXT PRIMARY KEY,
    goal_id TEXT NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',      -- pending | running | done | failed | skipped
    evidence TEXT NOT NULL DEFAULT '[]',         -- JSON array of evidence records
    started_at TEXT,
    finished_at TEXT
);
CREATE INDEX idx_goal_steps_goal ON goal_steps(goal_id, seq);

CREATE TABLE goal_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    goal_id TEXT NOT NULL,
    at TEXT NOT NULL,
    from_status TEXT NOT NULL,
    to_status TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT ''
);

CREATE TABLE memories (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,             -- session | project_fact | preference | procedure | correction | skill_ref | artifact_ref | goal_history
    content TEXT NOT NULL,
    source TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.5,
    scope TEXT NOT NULL DEFAULT 'project',       -- project | global
    workspace TEXT NOT NULL DEFAULT '',
    sensitivity TEXT NOT NULL DEFAULT 'normal',  -- normal | sensitive
    requires_approval INTEGER NOT NULL DEFAULT 0,
    approved INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    verified_at TEXT,
    expires_at TEXT
);

CREATE VIRTUAL TABLE memories_fts USING fts5(
    content,
    content='memories',
    content_rowid='rowid'
);

CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
END;
CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
    INSERT INTO memories_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TABLE skills (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    version TEXT NOT NULL,
    manifest TEXT NOT NULL,          -- JSON skill manifest
    enabled INTEGER NOT NULL DEFAULT 0,
    provenance TEXT NOT NULL DEFAULT 'user',     -- user | agent_proposed | imported
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE mcp_servers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    config TEXT NOT NULL,            -- JSON McpServerConfig
    trust TEXT NOT NULL DEFAULT 'untrusted',
    enabled INTEGER NOT NULL DEFAULT 0,
    last_health TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trace_id TEXT NOT NULL,
    session_id TEXT,
    at TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL            -- JSON, redacted before insert
);
CREATE INDEX idx_audit_trace ON audit_events(trace_id);
CREATE INDEX idx_audit_session ON audit_events(session_id);
CREATE INDEX idx_audit_kind ON audit_events(kind, at);

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,
    session_id TEXT,
    kind TEXT NOT NULL,              -- tool_output | download | diff | log
    path TEXT NOT NULL,              -- file under .nexus/state/artifacts/
    sha256 TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'text/plain',
    source_url TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE web_sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url TEXT NOT NULL,
    canonical_url TEXT,
    title TEXT NOT NULL DEFAULT '',
    retrieved_at TEXT NOT NULL,
    published_at TEXT,
    publisher TEXT,
    content_sha256 TEXT NOT NULL,
    excerpt TEXT NOT NULL DEFAULT '',
    reliability TEXT NOT NULL DEFAULT 'unrated',
    artifact_id TEXT
);

CREATE TABLE index_files (
    path TEXT PRIMARY KEY,           -- workspace-relative
    language TEXT NOT NULL DEFAULT '',
    size INTEGER NOT NULL,
    mtime_ms INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    indexed_at TEXT NOT NULL
);

CREATE TABLE index_symbols (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL REFERENCES index_files(path) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,              -- fn | struct | enum | trait | impl | class | method | const | type | mod
    line INTEGER NOT NULL,
    signature TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_symbols_name ON index_symbols(name);
CREATE INDEX idx_symbols_path ON index_symbols(path);

CREATE TABLE kv_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
