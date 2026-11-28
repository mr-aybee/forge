-- D-176: optimistic locking for service edits. Every save bumps the
-- version; an editor sends back the one it loaded and a mismatch is
-- refused (409) instead of overwriting a concurrent change.
ALTER TABLE services ADD COLUMN version BIGINT NOT NULL DEFAULT 0;
