-- D-154: a cluster's orchestration mode, chosen at creation.
-- Existing clusters keep Forge's own orchestration.
ALTER TABLE clusters ADD COLUMN mode TEXT NOT NULL DEFAULT 'default';
