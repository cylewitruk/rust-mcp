-- Earlier OSV imports could attribute a withdrawn alias to its RustSec ID.
-- The original ID is not recoverable from those rows; refresh before trusting them.
WITH affected AS (
    UPDATE advisory_matches SET requires_resync = TRUE
    WHERE source = 'rustsec_osv' AND affected_range LIKE 'withdrawn=%'
    RETURNING crate_id
)
UPDATE crates SET security_checked_at = NULL
WHERE id IN (SELECT crate_id FROM affected);

DELETE FROM query_cache;
