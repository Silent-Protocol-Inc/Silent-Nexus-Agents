-- Global, credential-free provider inventory cache. One row represents one
-- provider endpoint/auth-profile instance; inventory replacement is guarded by
-- monotonically increasing refresh generations.
CREATE TABLE IF NOT EXISTS provider_catalog_cache (
    instance_key TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL,
    endpoint_fingerprint TEXT NOT NULL,
    auth_profile_id TEXT NOT NULL,
    inventory_json TEXT NOT NULL DEFAULT '[]',
    health TEXT NOT NULL DEFAULT 'unknown'
        CHECK (health IN ('unknown', 'refreshing', 'healthy', 'stale', 'error')),
    refresh_generation INTEGER NOT NULL DEFAULT 0 CHECK (refresh_generation >= 0),
    latency_ms INTEGER,
    last_success_at TEXT,
    updated_at TEXT NOT NULL,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS idx_provider_catalog_provider
    ON provider_catalog_cache(provider_id, endpoint_fingerprint);
