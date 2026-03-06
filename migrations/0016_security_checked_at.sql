-- Track when each crate was last checked for security advisories so the
-- periodic sync can skip recently-checked crates instead of re-querying
-- every indexed crate on every startup.
ALTER TABLE crates ADD COLUMN IF NOT EXISTS security_checked_at TIMESTAMPTZ;
