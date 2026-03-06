-- Add contributor count to the existing GitHub metadata table.
ALTER TABLE github_repo_metadata
    ADD COLUMN IF NOT EXISTS contributor_count BIGINT;

-- GitHub release notes for indexed crates.
-- Stores the most recent releases per crate, refreshed alongside metadata.
CREATE TABLE IF NOT EXISTS github_releases (
    id BIGSERIAL PRIMARY KEY,
    crate_id BIGINT NOT NULL REFERENCES crates(id) ON DELETE CASCADE,
    tag_name TEXT NOT NULL,
    release_name TEXT,
    body TEXT,
    published_at TIMESTAMPTZ,
    prerelease BOOLEAN NOT NULL DEFAULT FALSE,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (crate_id, tag_name)
);

CREATE INDEX IF NOT EXISTS idx_github_releases_crate_id
    ON github_releases (crate_id);
