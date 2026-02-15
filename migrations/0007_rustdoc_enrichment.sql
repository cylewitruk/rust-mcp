-- ============================================================
-- 1. Column additions (all ALTER TABLEs before any index/table)
-- ============================================================

-- symbols: rustdoc item identity + import paths + deprecation
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS rustdoc_item_id INTEGER;
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS canonical_path TEXT;
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS definition_path TEXT;
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS deprecated_since TEXT;
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS deprecated_note TEXT;

-- crate_types: rustdoc enrichment columns
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS rustdoc_item_id INTEGER;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS canonical_path TEXT;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS definition_path TEXT;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS deprecated_since TEXT;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS deprecated_note TEXT;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS is_non_exhaustive BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS auto_traits JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS where_clauses JSONB NOT NULL DEFAULT '[]'::jsonb;

-- crate_impls: trait metadata
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS rustdoc_item_id INTEGER;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS is_blanket BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS is_synthetic BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS is_negative BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS blanket_type TEXT;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS generics JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS where_clauses JSONB NOT NULL DEFAULT '[]'::jsonb;

-- ============================================================
-- 2. New table
-- ============================================================

CREATE TABLE IF NOT EXISTS crate_traits (
    id BIGSERIAL PRIMARY KEY,
    crate_version_id BIGINT NOT NULL REFERENCES crate_versions(id) ON DELETE CASCADE,
    trait_name TEXT NOT NULL,
    is_auto BOOLEAN NOT NULL DEFAULT FALSE,
    is_unsafe BOOLEAN NOT NULL DEFAULT FALSE,
    is_dyn_compatible BOOLEAN NOT NULL DEFAULT FALSE,
    supertraits JSONB NOT NULL DEFAULT '[]'::jsonb,
    required_methods JSONB NOT NULL DEFAULT '[]'::jsonb,
    provided_methods JSONB NOT NULL DEFAULT '[]'::jsonb,
    associated_types JSONB NOT NULL DEFAULT '[]'::jsonb,
    generics JSONB NOT NULL DEFAULT '[]'::jsonb,
    index_source TEXT NOT NULL,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    rustdoc_item_id INTEGER,
    UNIQUE (crate_version_id, rustdoc_item_id, index_source)
);

-- ============================================================
-- 3. Indexes (all columns/tables now exist)
-- ============================================================

CREATE INDEX IF NOT EXISTS crate_traits_lookup_idx
    ON crate_traits (crate_version_id, trait_name);

-- Partial unique indexes: enforce one rustdoc row per item per crate version.
-- These are the authoritative identity constraint for rustdoc-sourced rows.
CREATE UNIQUE INDEX IF NOT EXISTS symbols_rustdoc_item_uniq
    ON symbols (crate_version_id, rustdoc_item_id)
    WHERE index_source = 'rustdoc_json' AND rustdoc_item_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS crate_types_rustdoc_item_uniq
    ON crate_types (crate_version_id, rustdoc_item_id)
    WHERE index_source = 'rustdoc_json' AND rustdoc_item_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS crate_impls_rustdoc_item_uniq
    ON crate_impls (crate_version_id, rustdoc_item_id)
    WHERE index_source = 'rustdoc_json' AND rustdoc_item_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS crate_traits_rustdoc_item_uniq
    ON crate_traits (crate_version_id, rustdoc_item_id)
    WHERE index_source = 'rustdoc_json' AND rustdoc_item_id IS NOT NULL;

-- ============================================================
-- 4. Legacy uniqueness: relax for rustdoc rows
-- ============================================================
-- The existing UNIQUE (crate_version_id, source_file_id, type_name, kind)
-- on crate_types (0006_type_intelligence.sql:15) will collide for rustdoc
-- rows: current ingestion uses one synthetic source_file_id per JSON blob,
-- so same-name types in different modules (e.g. foo::Error vs bar::Error)
-- hit the constraint. Drop the old unique index and replace with a partial
-- index that only constrains syn rows, while rustdoc rows use
-- rustdoc_item_id as their identity.
ALTER TABLE crate_types DROP CONSTRAINT IF EXISTS crate_types_crate_version_id_source_file_id_type_name_kind_key;
CREATE UNIQUE INDEX IF NOT EXISTS crate_types_syn_uniq
    ON crate_types (crate_version_id, source_file_id, type_name, kind)
    WHERE index_source != 'rustdoc_json';
