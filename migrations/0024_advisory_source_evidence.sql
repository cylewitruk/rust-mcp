-- Keep independent source evidence and quarantine legacy package-mixed matches.
ALTER TABLE advisory_matches ADD COLUMN requires_resync BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE advisory_matches ALTER COLUMN requires_resync SET DEFAULT FALSE;

DELETE FROM advisory_matches older USING advisory_matches newer
WHERE older.crate_id = newer.crate_id
  AND older.version_id IS NOT DISTINCT FROM newer.version_id
  AND older.advisory_id = newer.advisory_id
  AND older.source = newer.source
  AND older.id < newer.id;

ALTER TABLE advisory_matches
    DROP CONSTRAINT advisory_matches_crate_id_version_id_advisory_id_key;
ALTER TABLE advisory_matches
    ADD CONSTRAINT advisory_matches_source_key
    UNIQUE NULLS NOT DISTINCT (crate_id, version_id, advisory_id, source);

-- Startup security synchronization retries these crates before trusting their matches.
UPDATE crates SET security_checked_at = NULL
WHERE id IN (SELECT DISTINCT crate_id FROM advisory_matches);
DELETE FROM query_cache;
