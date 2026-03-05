-- Performance indexes for hot-path query patterns identified via SQL audit.

-- Covering index for the "latest version" lookup pattern used by every
-- tool via fetch_crate_context / fetch_latest_crate_version:
--   WHERE crate_id = $1 ORDER BY published_at DESC NULLS LAST, id DESC LIMIT 1
-- Also benefits: version timeline, alternatives LATERAL, graph DISTINCT ON,
-- dependent-crate lookups, and version history queries (9+ call sites).
-- The existing UNIQUE(crate_id, version) cannot satisfy the ORDER BY without a
-- sort step.
CREATE INDEX IF NOT EXISTS crate_versions_crate_id_latest_idx
    ON crate_versions (crate_id, published_at DESC NULLS LAST, id DESC);

-- Index for LEFT JOIN advisory_matches ON version_id patterns.
-- The existing UNIQUE(crate_id, version_id, advisory_id) has crate_id as the
-- leading column, making it inefficient for joins that filter solely on
-- version_id (version timeline, alternatives, intel).
CREATE INDEX IF NOT EXISTS advisory_matches_version_id_idx
    ON advisory_matches (version_id);

-- GIN indexes for array overlap (&&) and containment (@>) operators on
-- crates.categories and crates.keywords, used by crate_search and
-- crate_alternatives queries.
CREATE INDEX IF NOT EXISTS crates_categories_gin_idx
    ON crates USING GIN (categories);

CREATE INDEX IF NOT EXISTS crates_keywords_gin_idx
    ON crates USING GIN (keywords);
