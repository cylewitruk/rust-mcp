CREATE TABLE IF NOT EXISTS crate_version_features (
    id BIGSERIAL PRIMARY KEY,
    crate_version_id BIGINT NOT NULL REFERENCES crate_versions(id) ON DELETE CASCADE,
    feature_name TEXT NOT NULL,
    enables JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (crate_version_id, feature_name)
);

CREATE INDEX IF NOT EXISTS crate_version_features_version_idx
    ON crate_version_features (crate_version_id);

CREATE INDEX IF NOT EXISTS crate_version_features_name_idx
    ON crate_version_features USING GIN (feature_name gin_trgm_ops);
