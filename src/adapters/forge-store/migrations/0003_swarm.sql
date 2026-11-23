-- D-154: the swarm behind a Swarm cluster, and which nodes are in it.
CREATE TABLE cluster_swarms (
    cluster_id        TEXT PRIMARY KEY REFERENCES clusters(id),
    swarm_id          TEXT NOT NULL DEFAULT '',
    port              BIGINT NOT NULL DEFAULT 2377,
    default_addr_pool TEXT NOT NULL DEFAULT '[]',
    autolock          BIGINT NOT NULL DEFAULT 0,
    tokens_sealed     TEXT,
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
