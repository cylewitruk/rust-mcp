-- Durable session store so that MCP sessions survive server restarts.
-- When a client presents a session ID that exists in this table but has no
-- in-memory worker, the server lazily reconstitutes the session instead of
-- returning "Session not found".
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
