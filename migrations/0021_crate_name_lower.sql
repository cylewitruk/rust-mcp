-- Case-insensitive crate name lookups.
--
-- crates.io preserves author-chosen casing (e.g. "TinyUFO") but Cargo and
-- most tools normalise names to lowercase.  Add a stored generated column so
-- queries can match on `name_lower` without per-query LOWER() on the indexed
-- column, and without touching INSERT/UPDATE call-sites.

ALTER TABLE crates
    ADD COLUMN name_lower TEXT GENERATED ALWAYS AS (LOWER(name)) STORED;

CREATE UNIQUE INDEX crates_name_lower_idx ON crates (name_lower);

-- refresh_jobs stores crate names from user input that may have mixed case.
-- Normalise lookups by adding a matching generated column.
ALTER TABLE refresh_jobs
    ADD COLUMN crate_name_lower TEXT GENERATED ALWAYS AS (LOWER(crate_name)) STORED;

CREATE INDEX refresh_jobs_crate_name_lower_idx
    ON refresh_jobs (crate_name_lower);
