-- D-170: container settings of Docker object stores, and Forge's record of
-- the RustFS access keys it manages (a cache and audit trail; RustFS's own
-- IAM is the source of truth).
CREATE TABLE object_store_settings (
    store_id   TEXT PRIMARY KEY REFERENCES object_stores(id),
    settings   TEXT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE object_store_keys (
    id            TEXT PRIMARY KEY,
    store_id      TEXT NOT NULL REFERENCES object_stores(id),
    access_key    TEXT NOT NULL,
    name          TEXT NOT NULL,
    grants        TEXT,
    vault_item_id TEXT,
    created_by    TEXT NOT NULL,
    created_at    BIGINT NOT NULL,
    updated_at    BIGINT NOT NULL,
    synced_at     BIGINT,
    UNIQUE (store_id, access_key)
);
