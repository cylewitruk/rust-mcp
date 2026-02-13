ALTER TABLE crates
    ADD COLUMN IF NOT EXISTS last_checked_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS next_check_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS ttl_hint_seconds INTEGER NOT NULL DEFAULT 86400,
    ADD COLUMN IF NOT EXISTS ttl_reason TEXT,
    ADD COLUMN IF NOT EXISTS last_refresh_error TEXT;

CREATE TABLE IF NOT EXISTS refresh_jobs (
    id BIGSERIAL PRIMARY KEY,
    crate_name TEXT NOT NULL,
    scope TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 100,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    include_dependencies BOOLEAN NOT NULL DEFAULT TRUE,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS refresh_jobs_status_priority_idx
    ON refresh_jobs (status, priority, requested_at);

CREATE INDEX IF NOT EXISTS refresh_jobs_crate_scope_status_idx
    ON refresh_jobs (crate_name, scope, status);

CREATE INDEX IF NOT EXISTS crates_next_check_idx
    ON crates (next_check_at);
