-- Add indexes on source_file_id for symbols, crate_types, and crate_impls.
--
-- These columns have foreign keys to source_files(id) ON DELETE CASCADE but
-- lacked supporting indexes, causing sequential scans on COUNT, DELETE, and
-- cascading-delete operations during source file re-indexing.

CREATE INDEX IF NOT EXISTS symbols_source_file_id_idx
    ON symbols (source_file_id);

CREATE INDEX IF NOT EXISTS crate_types_source_file_id_idx
    ON crate_types (source_file_id);

CREATE INDEX IF NOT EXISTS crate_impls_source_file_id_idx
    ON crate_impls (source_file_id);
