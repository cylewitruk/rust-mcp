CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS unaccent;
CREATE EXTENSION IF NOT EXISTS btree_gin;

CREATE TABLE IF NOT EXISTS crates (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    repository_url TEXT,
    docs_url TEXT,
    homepage_url TEXT,
    categories TEXT[] NOT NULL DEFAULT '{}',
    keywords TEXT[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS crate_versions (
    id BIGSERIAL PRIMARY KEY,
    crate_id BIGINT NOT NULL REFERENCES crates(id) ON DELETE CASCADE,
    version TEXT NOT NULL,
    published_at TIMESTAMPTZ,
    yanked BOOLEAN NOT NULL DEFAULT FALSE,
    total_downloads BIGINT NOT NULL DEFAULT 0,
    rust_version TEXT,
    checksum TEXT,
    readme TEXT,
    readme_search TSVECTOR GENERATED ALWAYS AS (
        to_tsvector('english', COALESCE(readme, ''))
    ) STORED,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (crate_id, version)
);

CREATE TABLE IF NOT EXISTS dependency_edges (
    id BIGSERIAL PRIMARY KEY,
    from_version_id BIGINT NOT NULL REFERENCES crate_versions(id) ON DELETE CASCADE,
    to_crate_id BIGINT NOT NULL REFERENCES crates(id) ON DELETE CASCADE,
    requirement TEXT NOT NULL,
    dependency_kind TEXT NOT NULL,
    optional BOOLEAN NOT NULL DEFAULT FALSE,
    features JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS advisory_matches (
    id BIGSERIAL PRIMARY KEY,
    crate_id BIGINT NOT NULL REFERENCES crates(id) ON DELETE CASCADE,
    version_id BIGINT REFERENCES crate_versions(id) ON DELETE CASCADE,
    advisory_id TEXT NOT NULL,
    severity TEXT,
    title TEXT NOT NULL,
    url TEXT,
    affected_range TEXT NOT NULL,
    fixed_versions JSONB NOT NULL DEFAULT '[]'::jsonb,
    source TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (crate_id, version_id, advisory_id)
);

CREATE TABLE IF NOT EXISTS source_files (
    id BIGSERIAL PRIMARY KEY,
    crate_version_id BIGINT NOT NULL REFERENCES crate_versions(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    file_size BIGINT NOT NULL,
    language TEXT,
    content TEXT NOT NULL,
    content_search TSVECTOR GENERATED ALWAYS AS (
        to_tsvector('english', COALESCE(content, ''))
    ) STORED,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (crate_version_id, path)
);

CREATE TABLE IF NOT EXISTS symbols (
    id BIGSERIAL PRIMARY KEY,
    crate_version_id BIGINT NOT NULL REFERENCES crate_versions(id) ON DELETE CASCADE,
    source_file_id BIGINT NOT NULL REFERENCES source_files(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    signature TEXT,
    visibility TEXT,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    index_source TEXT NOT NULL,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS docs_pages (
    id BIGSERIAL PRIMARY KEY,
    crate_version_id BIGINT NOT NULL REFERENCES crate_versions(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    title TEXT,
    content TEXT NOT NULL,
    content_search TSVECTOR GENERATED ALWAYS AS (
        to_tsvector('english', COALESCE(content, ''))
    ) STORED,
    source_url TEXT,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (crate_version_id, path)
);

CREATE TABLE IF NOT EXISTS query_cache (
    key TEXT PRIMARY KEY,
    value JSONB NOT NULL,
    source TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS crates_name_trgm_idx ON crates USING GIN (name gin_trgm_ops);
CREATE INDEX IF NOT EXISTS crates_description_fts_idx ON crates USING GIN (
    to_tsvector('english', COALESCE(description, ''))
);

CREATE INDEX IF NOT EXISTS crate_versions_readme_fts_idx ON crate_versions USING GIN (readme_search);
CREATE INDEX IF NOT EXISTS dependency_edges_from_idx ON dependency_edges (from_version_id);
CREATE INDEX IF NOT EXISTS dependency_edges_to_idx ON dependency_edges (to_crate_id);

CREATE INDEX IF NOT EXISTS source_files_path_trgm_idx ON source_files USING GIN (path gin_trgm_ops);
CREATE INDEX IF NOT EXISTS source_files_content_fts_idx ON source_files USING GIN (content_search);

CREATE INDEX IF NOT EXISTS symbols_name_trgm_idx ON symbols USING GIN (name gin_trgm_ops);
CREATE INDEX IF NOT EXISTS symbols_crate_kind_name_idx ON symbols (crate_version_id, kind, name);

CREATE INDEX IF NOT EXISTS docs_pages_content_fts_idx ON docs_pages USING GIN (content_search);
CREATE INDEX IF NOT EXISTS query_cache_expires_idx ON query_cache (expires_at);
