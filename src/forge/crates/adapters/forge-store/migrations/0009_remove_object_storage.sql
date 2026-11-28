-- D-180: Object Storage is removed; a running container can be adopted as a
-- Service instead.
--
-- The container name an adopted service keeps using until its first deploy
-- replaces it with the normal `forge-app-<env>-<svc>` container. NULL for
-- every other service.
ALTER TABLE services ADD COLUMN adopted_from TEXT;

-- The containers of managed Docker object stores, one row per member node,
-- waiting to be adopted as services by the server (it reads each container
-- through the node's agent once the node is online, then deletes the row).
-- `settings` is the store's container settings JSON (it names the container),
-- or NULL for the defaults. Native and connected stores are not adopted.
CREATE TABLE object_store_adoptions (
    id             TEXT PRIMARY KEY,
    workspace_id   TEXT NOT NULL,
    environment_id TEXT,
    server_id      TEXT NOT NULL,
    slug           TEXT NOT NULL,
    settings       TEXT,
    console_port   BIGINT,
    credential_id  TEXT NOT NULL,
    created_by     TEXT NOT NULL,
    created_at     BIGINT NOT NULL
);

INSERT INTO object_store_adoptions
    (id, workspace_id, environment_id, server_id, slug, settings, console_port, credential_id, created_by, created_at)
SELECT s.id || ':' || m.server_id, s.workspace_id, s.environment_id, m.server_id, s.slug, st.settings,
       s.console_port, s.credential_id, s.created_by, s.created_at
FROM object_stores s
JOIN object_store_members m ON m.store_id = s.id
LEFT JOIN object_store_settings st ON st.store_id = s.id
WHERE s.mode = 'managed' AND s.runtime = 'docker';

-- Dependents first, so foreign keys never point at a dropped table.
DROP TABLE bucket_grants;
DROP TABLE object_store_keys;
DROP TABLE object_store_settings;
DROP TABLE object_store_members;
DROP TABLE object_stores;
