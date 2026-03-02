-- Source file content is now read directly from the cargo registry on disk.
-- The source_files table retains metadata (path, sha256, file_size, language,
-- indexed_at) but no longer stores the file body.
ALTER TABLE source_files DROP COLUMN IF EXISTS content;
