-- Track rustdoc JSON enrichment state per crate version so that the
-- enrichment maintenance task can skip already-enriched versions and
-- respect a retry cooldown for failed attempts.

ALTER TABLE crate_versions
    ADD COLUMN IF NOT EXISTS rustdoc_enriched_at TIMESTAMPTZ;

ALTER TABLE crate_versions
    ADD COLUMN IF NOT EXISTS rustdoc_last_attempt_at TIMESTAMPTZ;

-- Partial index: maintenance queries filter WHERE rustdoc_enriched_at IS NULL.
CREATE INDEX IF NOT EXISTS crate_versions_rustdoc_unenriched_idx
    ON crate_versions (crate_id)
    WHERE rustdoc_enriched_at IS NULL;
