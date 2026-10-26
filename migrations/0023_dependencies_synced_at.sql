-- Record when a crate version's dependency list was fetched from crates.io, so
-- an empty dependency list can be told apart from one that was never synced.
--
-- Versions that already have dependency edges were synced; versions synced
-- with zero dependencies cannot be recovered and stay NULL until re-synced.

ALTER TABLE crate_versions
    ADD COLUMN dependencies_synced_at TIMESTAMPTZ;

UPDATE crate_versions cv
SET dependencies_synced_at = NOW()
WHERE EXISTS (SELECT 1 FROM dependency_edges de WHERE de.from_version_id = cv.id);
