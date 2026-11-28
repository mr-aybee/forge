-- Container platform (D-154 … D-160): cluster modes, Docker Swarm, registries
-- and deploy webhooks. One migration for the whole feature branch.

-- ---- cluster modes ----
-- D-154: a cluster's orchestration mode, chosen at creation.
-- Existing clusters keep Forge's own orchestration.
ALTER TABLE clusters ADD COLUMN mode TEXT NOT NULL DEFAULT 'default';

-- ---- docker swarm ----
-- D-154: the swarm behind a Swarm cluster, and which nodes are in it.
CREATE TABLE cluster_swarms (
    cluster_id        TEXT PRIMARY KEY REFERENCES clusters(id),
    swarm_id          TEXT NOT NULL DEFAULT '',
    port              BIGINT NOT NULL DEFAULT 2377,
    default_addr_pool TEXT NOT NULL DEFAULT '[]',
    autolock          BIGINT NOT NULL DEFAULT 0,
    tokens_sealed     TEXT,
    -- The node creating the swarm while swarm_id is still empty: claiming
    -- this row is what makes only one node create it.
    founder           TEXT,
    updated_at        BIGINT NOT NULL
);

CREATE TABLE swarm_members (
    server_id      TEXT PRIMARY KEY REFERENCES servers(id),
    cluster_id     TEXT NOT NULL REFERENCES clusters(id),
    node_id        TEXT NOT NULL DEFAULT '',
    role           TEXT NOT NULL,
    advertise_addr TEXT NOT NULL DEFAULT '',
    manager_addr   TEXT NOT NULL DEFAULT '',
    state          TEXT NOT NULL,
    error          TEXT NOT NULL DEFAULT '',
    updated_at     BIGINT NOT NULL
);

CREATE INDEX swarm_members_cluster ON swarm_members (cluster_id);
CREATE INDEX cluster_swarms_swarm ON cluster_swarms (swarm_id);

-- ---- registries ----
-- D-158: container registries per workspace; the secret is sealed.
CREATE TABLE registries (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id),
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL,
    host          TEXT NOT NULL,
    username      TEXT NOT NULL DEFAULT '',
    secret_sealed TEXT NOT NULL,
    -- Pushes need this on, besides registry.manage (off by default).
    allow_push    BIGINT NOT NULL DEFAULT 0,
    created_by    TEXT NOT NULL,
    created_at    BIGINT NOT NULL,
    updated_at    BIGINT NOT NULL,
    UNIQUE (workspace_id, host)
);

-- ---- deploy webhooks ----
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
CREATE INDEX webhooks_workspace ON webhooks (workspace_id);

