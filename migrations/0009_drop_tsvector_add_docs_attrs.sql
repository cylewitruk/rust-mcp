-- Drop unused TSVECTOR columns and their GIN indexes.
-- No query uses these; all searches use ILIKE / ~* on the raw content column.
DROP INDEX IF EXISTS source_files_content_fts_idx;
ALTER TABLE source_files DROP COLUMN IF EXISTS content_search;

DROP INDEX IF EXISTS docs_pages_content_fts_idx;
ALTER TABLE docs_pages DROP COLUMN IF EXISTS content_search;

-- Make source_files.content nullable so rustdoc_json rows can omit the blob.
ALTER TABLE source_files ALTER COLUMN content DROP NOT NULL;

-- Per-symbol/type documentation (raw markdown from rustdoc item.docs).
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS docs TEXT;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS docs TEXT;
ALTER TABLE crate_impls ADD COLUMN IF NOT EXISTS docs TEXT;
ALTER TABLE crate_traits ADD COLUMN IF NOT EXISTS docs TEXT;

-- Structured attributes (JSONB array from rustdoc item.attrs).
ALTER TABLE symbols ADD COLUMN IF NOT EXISTS attrs JSONB;
ALTER TABLE crate_types ADD COLUMN IF NOT EXISTS attrs JSONB;

-- Reclaim storage for existing rustdoc JSON blobs.
UPDATE source_files SET content = NULL WHERE language = 'rustdoc_json';
