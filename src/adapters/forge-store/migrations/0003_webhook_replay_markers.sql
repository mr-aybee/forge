-- D-168: durable replay protection for deploy webhooks. A delivery's marker
-- (the SHA-256 of its signature or CI delivery id) is claimed once per
-- webhook; rows older than the retention window are pruned on each claim.
CREATE TABLE webhook_replay_markers (
    webhook_id TEXT NOT NULL REFERENCES webhooks(id),
    marker     TEXT NOT NULL,
    at         BIGINT NOT NULL,
    PRIMARY KEY (webhook_id, marker)
);

CREATE INDEX webhook_replay_markers_at ON webhook_replay_markers (at);
