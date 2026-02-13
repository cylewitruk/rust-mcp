CREATE TABLE IF NOT EXISTS tool_invocations (
    id BIGSERIAL PRIMARY KEY,
    tool_name TEXT NOT NULL,
    success BOOLEAN NOT NULL,
    latency_ms BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS query_cache_events (
    id BIGSERIAL PRIMARY KEY,
    source TEXT NOT NULL,
    hit BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS tool_invocations_created_at_idx
    ON tool_invocations (created_at DESC);

CREATE INDEX IF NOT EXISTS tool_invocations_tool_name_idx
    ON tool_invocations (tool_name, created_at DESC);

CREATE INDEX IF NOT EXISTS query_cache_events_created_at_idx
    ON query_cache_events (created_at DESC);

CREATE INDEX IF NOT EXISTS query_cache_events_source_idx
    ON query_cache_events (source, created_at DESC);
