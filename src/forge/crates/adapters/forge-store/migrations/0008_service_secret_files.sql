-- D-179: services mount credential vault items as files. When a referenced
-- item is rotated, the service needs a redeploy: the epoch ms of that change,
-- or NULL when the running copies have the current values. A deploy that
-- started after the change clears it.
ALTER TABLE services ADD COLUMN secrets_changed_at BIGINT;
