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
