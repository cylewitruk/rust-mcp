-- Add commit liveness columns to github_repo_metadata.
-- These are populated by the git probe (shallow bare clone), not by the
-- GitHub API, so they don't consume API rate limits.

ALTER TABLE github_repo_metadata
    ADD COLUMN IF NOT EXISTS last_commit_at    TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS last_commit_message TEXT,
    ADD COLUMN IF NOT EXISTS recent_commit_count BIGINT;
