-- Track which crate versions are physically present in the user's cargo
-- registry on disk. Proactive enrichment (source + rustdoc) is limited to
-- locally-present versions to avoid indexing hundreds of historical versions
-- that the user doesn't have.

ALTER TABLE crate_versions
    ADD COLUMN IF NOT EXISTS locally_present BOOLEAN NOT NULL DEFAULT FALSE;

-- Partial index: enrichment queries filter WHERE locally_present = TRUE.
CREATE INDEX IF NOT EXISTS crate_versions_locally_present_idx
    ON crate_versions (crate_id)
    WHERE locally_present = TRUE;
