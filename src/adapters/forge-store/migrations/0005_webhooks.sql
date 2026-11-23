-- D-159: deploy webhooks and their recent deliveries.
CREATE TABLE webhooks (
    id               TEXT PRIMARY KEY,
    workspace_id     TEXT NOT NULL REFERENCES workspaces(id),
    name             TEXT NOT NULL,
    target           TEXT NOT NULL,
    token_hash       TEXT NOT NULL UNIQUE,
    token_hint       TEXT NOT NULL,
    signature        TEXT NOT NULL,
    secret_sealed    TEXT,
    tag_pattern      TEXT,
    enabled          BIGINT NOT NULL DEFAULT 1,
    created_by       TEXT NOT NULL,
    created_at       BIGINT NOT NULL,
    updated_at       BIGINT NOT NULL,
    last_delivery_at BIGINT
);

CREATE TABLE webhook_deliveries (
    id         TEXT PRIMARY KEY,
    webhook_id TEXT NOT NULL REFERENCES webhooks(id),
    at         BIGINT NOT NULL,
    outcome    TEXT NOT NULL,
    reason     TEXT NOT NULL DEFAULT '',
    job_id     TEXT,
    tag        TEXT,
    source_ip  TEXT NOT NULL DEFAULT ''
);

CREATE INDEX webhook_deliveries_hook ON webhook_deliveries (webhook_id, at);
