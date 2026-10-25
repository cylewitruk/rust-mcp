-- Legacy-protocol sessions now persist the client's original `initialize`
-- parameters (rmcp `SessionState`), so a restored session replays the real
-- handshake instead of a synthetic one.
--
-- Rows written by the previous format carry no state and cannot be restored;
-- their clients receive 404 and re-initialize, as the spec requires.

DELETE FROM sessions;

ALTER TABLE sessions
    ADD COLUMN state JSONB NOT NULL;
