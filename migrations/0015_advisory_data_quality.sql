-- Add richer advisory data columns: details, affected functions, CWE IDs.

ALTER TABLE advisory_matches ADD COLUMN IF NOT EXISTS details TEXT;
ALTER TABLE advisory_matches ADD COLUMN IF NOT EXISTS affected_functions TEXT[] NOT NULL DEFAULT '{}';
ALTER TABLE advisory_matches ADD COLUMN IF NOT EXISTS cwe_ids TEXT[] NOT NULL DEFAULT '{}';
