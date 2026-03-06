-- GitHub repository metadata for indexed crates.
-- Populated on-demand from the GitHub REST API when a crate has a
-- github.com repository_url.
CREATE TABLE IF NOT EXISTS github_repo_metadata (
    id BIGSERIAL PRIMARY KEY,
    crate_id BIGINT NOT NULL UNIQUE REFERENCES crates(id) ON DELETE CASCADE,
    owner TEXT NOT NULL,
    repo TEXT NOT NULL,
    stargazers_count BIGINT NOT NULL DEFAULT 0,
    forks_count BIGINT NOT NULL DEFAULT 0,
    open_issues_count BIGINT NOT NULL DEFAULT 0,
    archived BOOLEAN NOT NULL DEFAULT FALSE,
    pushed_at TIMESTAMPTZ,
    license_spdx TEXT,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
