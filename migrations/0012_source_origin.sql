ALTER TABLE crate_versions
    ADD COLUMN IF NOT EXISTS source_origin SMALLINT NOT NULL DEFAULT 0;
-- 0 = none (source not yet available)
-- 1 = host_registry (from mounted cargo cache)
-- 2 = downloaded (fetched from crates.io by rust-mcp)
