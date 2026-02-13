CREATE TABLE IF NOT EXISTS crate_types (
    id BIGSERIAL PRIMARY KEY,
    crate_version_id BIGINT NOT NULL REFERENCES crate_versions(id) ON DELETE CASCADE,
    source_file_id BIGINT NOT NULL REFERENCES source_files(id) ON DELETE CASCADE,
    type_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    visibility TEXT,
    generic_params JSONB NOT NULL DEFAULT '[]'::jsonb,
    fields JSONB NOT NULL DEFAULT '[]'::jsonb,
    variants JSONB NOT NULL DEFAULT '[]'::jsonb,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    index_source TEXT NOT NULL,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (crate_version_id, source_file_id, type_name, kind)
);

CREATE TABLE IF NOT EXISTS crate_impls (
    id BIGSERIAL PRIMARY KEY,
    crate_version_id BIGINT NOT NULL REFERENCES crate_versions(id) ON DELETE CASCADE,
    source_file_id BIGINT NOT NULL REFERENCES source_files(id) ON DELETE CASCADE,
    type_name TEXT NOT NULL,
    type_name_display TEXT,
    trait_name TEXT,
    trait_name_display TEXT,
    impl_kind TEXT NOT NULL,
    methods JSONB NOT NULL DEFAULT '[]'::jsonb,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    index_source TEXT NOT NULL,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS crate_types_lookup_idx
    ON crate_types (crate_version_id, type_name);

CREATE INDEX IF NOT EXISTS crate_types_kind_idx
    ON crate_types (crate_version_id, kind);

CREATE INDEX IF NOT EXISTS crate_impls_type_lookup_idx
    ON crate_impls (crate_version_id, type_name);

CREATE INDEX IF NOT EXISTS crate_impls_trait_lookup_idx
    ON crate_impls (crate_version_id, trait_name);

CREATE INDEX IF NOT EXISTS crate_impls_type_trait_idx
    ON crate_impls (crate_version_id, type_name, trait_name);
