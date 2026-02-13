ALTER TABLE crate_versions
ADD COLUMN IF NOT EXISTS license_expression TEXT;

CREATE INDEX IF NOT EXISTS crate_versions_license_expression_trgm_idx
    ON crate_versions USING GIN (license_expression gin_trgm_ops);
