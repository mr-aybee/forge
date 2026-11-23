-- Orbyn Forge schema (initial migration).
--
-- Every table and index a fresh install needs, in dependency order and grouped
-- by feature. The baseline: every earlier migration was squashed into this file; their
-- history is in git. Portable SQL for SQLite and PostgreSQL: TEXT and
-- BIGINT only, timestamps in epoch milliseconds, flags as 0/1.
--
-- This is the baseline. Add schema changes as new files after it (0002_…).


-- ======================================================================
-- Core: workspaces, tenancy, identity, nodes, jobs, audit
-- ======================================================================

CREATE TABLE workspaces (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    slug       TEXT NOT NULL UNIQUE,
    created_at BIGINT NOT NULL
);

CREATE TABLE tenants (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    name         TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    UNIQUE (workspace_id, name)
);

CREATE TABLE environment_scopes (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    tenant_id    TEXT REFERENCES tenants(id),
    name         TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    UNIQUE (workspace_id, name)
);

CREATE TABLE environments (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    scope_id     TEXT NOT NULL REFERENCES environment_scopes(id),
    name         TEXT NOT NULL,
    protected    BIGINT NOT NULL DEFAULT 0,
    created_at   BIGINT NOT NULL,
    cluster_id   TEXT,
    project_id TEXT NULL,
    UNIQUE (scope_id, name)
);
CREATE INDEX environments_cluster ON environments (cluster_id);

CREATE TABLE users (
    id                 TEXT PRIMARY KEY,
    username           TEXT UNIQUE,
    email              TEXT UNIQUE,
    display_name       TEXT NOT NULL,
    password_hash      TEXT,
    totp_secret_sealed TEXT,
    oidc_subject       TEXT UNIQUE,
    disabled           BIGINT NOT NULL DEFAULT 0,
    created_at         BIGINT NOT NULL,
    kind               TEXT NOT NULL DEFAULT 'person'
);

CREATE TABLE role_bindings (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id),
    role       TEXT NOT NULL,
    scope_kind TEXT NOT NULL,
    scope_id   TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE (user_id, role, scope_kind, scope_id)
);

CREATE TABLE sessions (
    id_hash       TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL REFERENCES users(id),
    kind          TEXT NOT NULL,
    csrf_hash     TEXT,
    refresh_hash  TEXT UNIQUE,
    mfa           BIGINT NOT NULL DEFAULT 0,
    created_at    BIGINT NOT NULL,
    expires_at    BIGINT NOT NULL,
    connection_id TEXT
);
CREATE INDEX sessions_user ON sessions (user_id);
CREATE INDEX sessions_connection ON sessions (connection_id);

CREATE TABLE oauth_codes (
    code_hash      TEXT PRIMARY KEY,
    user_id        TEXT NOT NULL REFERENCES users(id),
    client_id      TEXT NOT NULL,
    redirect_uri   TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    mfa            BIGINT NOT NULL DEFAULT 0,
    expires_at     BIGINT NOT NULL,
    connection_id  TEXT
);

CREATE TABLE servers (
    id             TEXT PRIMARY KEY,
    workspace_id   TEXT NOT NULL REFERENCES workspaces(id),
    environment_id   TEXT REFERENCES environments(id),
    hostname       TEXT NOT NULL,
    os             TEXT NOT NULL,
    arch           TEXT NOT NULL,
    agent_version  TEXT NOT NULL,
    status         TEXT NOT NULL,
    last_seen_at   BIGINT,
    cert_serial    TEXT,
    cert_not_after BIGINT,
    created_at     BIGINT NOT NULL,
    cluster_id     TEXT REFERENCES clusters(id),
    role           TEXT NOT NULL DEFAULT 'worker',
    display_name   TEXT
);
CREATE INDEX servers_org ON servers (workspace_id);
CREATE INDEX servers_cluster ON servers (cluster_id);
CREATE INDEX servers_status_seen ON servers (status, last_seen_at);

CREATE TABLE enrollment_tokens (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    environment_id TEXT REFERENCES environments(id),
    token_hash   TEXT NOT NULL UNIQUE,
    created_by   TEXT NOT NULL,
    expires_at   BIGINT NOT NULL,
    used_at      BIGINT,
    server_id    TEXT
);

CREATE TABLE revoked_certificates (
    serial     TEXT PRIMARY KEY,
    server_id  TEXT NOT NULL,
    revoked_at BIGINT NOT NULL
);

CREATE TABLE jobs (
    id               TEXT PRIMARY KEY,
    kind             TEXT NOT NULL,
    workspace_id     TEXT,
    resource         TEXT NOT NULL,
    principal        TEXT NOT NULL,
    authz_mfa        BIGINT NOT NULL DEFAULT 0,
    idempotency_key  TEXT,
    payload          TEXT NOT NULL,
    state            TEXT NOT NULL,
    status           TEXT NOT NULL,
    current_step     BIGINT NOT NULL DEFAULT 0,
    attempts         BIGINT NOT NULL DEFAULT 0,
    max_attempts     BIGINT NOT NULL,
    next_run_at      BIGINT NOT NULL,
    lease_owner      TEXT,
    lease_expires_at BIGINT,
    lease_token      BIGINT NOT NULL DEFAULT 0,
    last_error       TEXT,
    created_at       BIGINT NOT NULL,
    updated_at       BIGINT NOT NULL,
    UNIQUE (kind, idempotency_key)
);
CREATE INDEX jobs_runnable ON jobs (status, next_run_at);
CREATE INDEX jobs_org ON jobs (workspace_id, created_at);

CREATE TABLE job_steps (
    job_id      TEXT NOT NULL REFERENCES jobs(id),
    step_index  BIGINT NOT NULL,
    name        TEXT NOT NULL,
    status      TEXT NOT NULL,
    started_at  BIGINT NOT NULL,
    finished_at BIGINT,
    error       TEXT,
    PRIMARY KEY (job_id, step_index)
);

CREATE TABLE audit_log (
    seq          BIGINT PRIMARY KEY,
    id           TEXT NOT NULL UNIQUE,
    at           BIGINT NOT NULL,
    actor        TEXT NOT NULL,
    action       TEXT NOT NULL,
    resource     TEXT NOT NULL,
    workspace_id TEXT,
    outcome      TEXT NOT NULL,
    details      TEXT NOT NULL,
    prev_hash    TEXT NOT NULL,
    hash         TEXT NOT NULL,
    signature    TEXT NOT NULL
);
CREATE INDEX audit_org ON audit_log (workspace_id, seq);

CREATE TABLE plugins (
    plugin_id            TEXT PRIMARY KEY,
    version              TEXT NOT NULL,
    publisher            TEXT NOT NULL,
    manifest_toml        TEXT NOT NULL,
    artifact_sha256      TEXT NOT NULL,
    state                TEXT NOT NULL,
    pinned               BIGINT NOT NULL DEFAULT 0,
    approved_permissions TEXT NOT NULL,
    fault_count          BIGINT NOT NULL DEFAULT 0,
    quarantine_reason    TEXT,
    installed_at         BIGINT NOT NULL,
    updated_at           BIGINT NOT NULL
);

CREATE TABLE clusters (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    name         TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    environment_id TEXT REFERENCES environments(id),
    created_at   BIGINT NOT NULL,
    abbr         TEXT NOT NULL DEFAULT '',
    icon         TEXT,
    UNIQUE (workspace_id, name)
);

CREATE TABLE secrets (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    kind         TEXT NOT NULL,
    label        TEXT NOT NULL,
    sealed       TEXT NOT NULL,
    expires_at   BIGINT,
    created_by   TEXT NOT NULL,
    created_at   BIGINT NOT NULL
);
CREATE INDEX secrets_expiry ON secrets (expires_at);

CREATE TABLE node_ssh (
    server_id  TEXT PRIMARY KEY REFERENCES servers(id),
    host       TEXT NOT NULL,
    port       BIGINT NOT NULL,
    host_key   TEXT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE installations (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id),
    server_id     TEXT NOT NULL REFERENCES servers(id),
    package       TEXT NOT NULL,
    version       TEXT NOT NULL,
    params        TEXT NOT NULL,
    status        TEXT NOT NULL,
    state         TEXT,
    env           TEXT NOT NULL,
    last_job_id   TEXT,
    last_error    TEXT,
    created_at    BIGINT NOT NULL,
    updated_at    BIGINT NOT NULL,
    versions      TEXT,
    update_policy TEXT NOT NULL DEFAULT 'notify',
    UNIQUE (server_id, package, version)
);
CREATE INDEX installations_server ON installations (server_id);

CREATE TABLE services (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    environment_id TEXT NOT NULL REFERENCES environments(id),
    name         TEXT NOT NULL,
    runtime      TEXT NOT NULL,
    spec         TEXT NOT NULL,
    env          TEXT NOT NULL,
    created_by   TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL,
    UNIQUE (environment_id, name)
);

CREATE TABLE service_instances (
    service_id      TEXT NOT NULL REFERENCES services(id),
    server_id   TEXT NOT NULL REFERENCES servers(id),
    role        TEXT NOT NULL,
    status      TEXT NOT NULL,
    state       TEXT,
    last_job_id TEXT,
    last_error  TEXT,
    updated_at  BIGINT NOT NULL,
    PRIMARY KEY (service_id, server_id)
);
CREATE INDEX service_instances_server ON service_instances (server_id);

CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE api_keys (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users(id),
    name         TEXT NOT NULL,
    prefix       TEXT NOT NULL,
    secret_hash  TEXT NOT NULL UNIQUE,
    expires_at   BIGINT,
    last_used_at BIGINT,
    created_at   BIGINT NOT NULL,
    elevated BIGINT NOT NULL DEFAULT 0,
    restrictions TEXT
);
CREATE INDEX api_keys_user ON api_keys (user_id);

CREATE TABLE sites (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    name         TEXT NOT NULL UNIQUE,
    config       TEXT NOT NULL,
    edge_status  TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL
);

CREATE TABLE cluster_profiles (
    cluster_id TEXT PRIMARY KEY REFERENCES clusters(id),
    profile    TEXT NOT NULL,
    status     TEXT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE cluster_fabrics (
    cluster_id TEXT PRIMARY KEY REFERENCES clusters(id),
    spec       TEXT NOT NULL,
    pins       TEXT NOT NULL,
    leases     TEXT NOT NULL,
    status     TEXT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE vault_items (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    name         TEXT NOT NULL,
    username     TEXT NOT NULL,
    url          TEXT NOT NULL,
    notes        TEXT NOT NULL,
    tags         TEXT NOT NULL,
    links        TEXT NOT NULL,
    sealed       TEXT NOT NULL,
    created_by   TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL,
    rotated_at   BIGINT NOT NULL
);
CREATE INDEX vault_items_workspace ON vault_items (workspace_id);

CREATE TABLE tls_certificates (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    name         TEXT NOT NULL,
    domains      TEXT NOT NULL,
    issuer       TEXT NOT NULL,
    not_before   BIGINT NOT NULL,
    not_after    BIGINT NOT NULL,
    fingerprint  TEXT NOT NULL,
    chain_pem    TEXT NOT NULL,
    sealed_key   TEXT NOT NULL,
    created_by   TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL
);
CREATE INDEX tls_certificates_workspace ON tls_certificates (workspace_id);

CREATE TABLE package_releases (
    product       TEXT NOT NULL,
    line          TEXT NOT NULL,
    latest        TEXT NOT NULL,
    lts           BIGINT NOT NULL,
    released_at   TEXT,
    latest_at     TEXT,
    eol           TEXT,
    support_until TEXT,
    fetched_at    BIGINT NOT NULL,
    PRIMARY KEY (product, line)
);


-- ======================================================================
-- Object storage
-- ======================================================================

CREATE TABLE object_stores (
    id               TEXT PRIMARY KEY,
    workspace_id     TEXT NOT NULL REFERENCES workspaces(id),
    name             TEXT NOT NULL,
    slug             TEXT NOT NULL,
    provider         TEXT NOT NULL,
    mode             TEXT NOT NULL,
    region           TEXT NOT NULL,
    path_style       BIGINT NOT NULL,
    version          TEXT NOT NULL,
    deployed_version TEXT,
    runtime          TEXT NOT NULL,
    topology         TEXT NOT NULL,
    s3_port          BIGINT NOT NULL,
    console_port     BIGINT,
    endpoint         TEXT NOT NULL,
    credential_id    TEXT NOT NULL,
    status           TEXT NOT NULL,
    last_error       TEXT,
    last_job_id      TEXT,
    created_by       TEXT NOT NULL,
    created_at       BIGINT NOT NULL,
    updated_at       BIGINT NOT NULL,
    environment_id TEXT NULL
);
CREATE INDEX object_stores_workspace ON object_stores (workspace_id);

CREATE TABLE object_store_members (
    store_id       TEXT NOT NULL REFERENCES object_stores(id) ON DELETE CASCADE,
    server_id      TEXT NOT NULL REFERENCES servers(id),
    data_paths     TEXT NOT NULL,
    endpoint_host  TEXT NOT NULL,
    require_mount  BIGINT NOT NULL,
    status         TEXT NOT NULL,
    capacity_bytes BIGINT,
    free_bytes     BIGINT,
    dedicated      BIGINT,
    observed_at    BIGINT,
    PRIMARY KEY (store_id, server_id)
);
CREATE INDEX object_store_members_server ON object_store_members (server_id);


-- ======================================================================
-- File explorer
-- ======================================================================

CREATE TABLE file_roots (
    id             TEXT PRIMARY KEY,
    workspace_id   TEXT NOT NULL REFERENCES workspaces(id),
    server_id      TEXT NOT NULL REFERENCES servers(id),
    kind           TEXT NOT NULL,
    label          TEXT NOT NULL,
    spec           TEXT NOT NULL,
    read_only      BIGINT NOT NULL,
    service_id TEXT,
    created_by     TEXT NOT NULL,
    created_at     BIGINT NOT NULL
);
CREATE INDEX file_roots_server ON file_roots (server_id);

CREATE TABLE file_uploads (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    server_id    TEXT NOT NULL REFERENCES servers(id),
    root_id      TEXT NOT NULL,
    path         TEXT NOT NULL,
    size         BIGINT NOT NULL,
    received     BIGINT NOT NULL,
    overwrite    TEXT NOT NULL,
    created_by   TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    expires_at   BIGINT NOT NULL
);
CREATE INDEX file_uploads_expiry ON file_uploads (expires_at);


-- ======================================================================
-- Remote access
-- ======================================================================




-- ======================================================================
-- Identity and onboarding
-- ======================================================================

CREATE TABLE enrollment_targets (
    token_id   TEXT PRIMARY KEY,
    cluster_id TEXT REFERENCES clusters(id) ON DELETE CASCADE,
    hostname   TEXT,
    created_at BIGINT NOT NULL
);


-- ======================================================================
-- Passkeys
-- ======================================================================

CREATE TABLE passkeys (
    id            TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- base64url credential id; unique across Forge.
    credential_id TEXT NOT NULL UNIQUE,
    -- forge_identity::webauthn::PublicKey as JSON (alg + key material).
    public_key    TEXT NOT NULL,
    sign_count    BIGINT NOT NULL,
    name          TEXT NOT NULL,
    created_at    BIGINT NOT NULL,
    last_used_at  BIGINT
);
CREATE INDEX passkeys_user ON passkeys (user_id);

CREATE TABLE webauthn_challenges (
    challenge_hash TEXT PRIMARY KEY,
    -- Set for registration and step-up; empty for passwordless sign-in.
    user_id        TEXT,
    -- `register`, `login` or `step_up`.
    purpose        TEXT NOT NULL,
    expires_at     BIGINT NOT NULL
);
CREATE INDEX webauthn_challenges_expiry ON webauthn_challenges (expires_at);


-- ======================================================================
-- SFTP grants
-- ======================================================================

CREATE TABLE file_grants (
    id                TEXT PRIMARY KEY,
    workspace_id      TEXT NOT NULL REFERENCES workspaces(id),
    server_id         TEXT NOT NULL REFERENCES servers(id),
    name              TEXT NOT NULL,
    roots             TEXT NOT NULL,
    can_write         BIGINT NOT NULL,
    can_delete        BIGINT NOT NULL,
    token_hash        TEXT NOT NULL,
    created_by        TEXT NOT NULL,
    created_at        BIGINT NOT NULL,
    expires_at        BIGINT NOT NULL,
    revoked_at        BIGINT,
    allowed_ips       TEXT NOT NULL,
    max_sessions      BIGINT NOT NULL,
    bandwidth_kbps    BIGINT,
    transfer_limit    BIGINT,
    bytes_transferred BIGINT NOT NULL,
    last_used_at      BIGINT,
    owner             TEXT
);
CREATE INDEX file_grants_server ON file_grants (server_id);


-- ======================================================================
-- Node tags and facts
-- ======================================================================

CREATE TABLE entity_tags (
    kind       TEXT NOT NULL,
    entity_id  TEXT NOT NULL,
    tag        TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (kind, entity_id, tag)
);
CREATE INDEX entity_tags_by_tag ON entity_tags (kind, tag);

CREATE TABLE server_facts (
    server_id    TEXT PRIMARY KEY,
    facts        TEXT NOT NULL,
    collected_at BIGINT NOT NULL
);

CREATE TABLE server_logins (
    server_id  TEXT NOT NULL,
    login      TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (server_id, login)
);


-- ======================================================================
-- Compose stacks
-- ======================================================================

CREATE TABLE stacks (
    id                TEXT PRIMARY KEY,
    workspace_id      TEXT NOT NULL REFERENCES workspaces(id),
    environment_id      TEXT NOT NULL REFERENCES environments(id),
    name              TEXT NOT NULL,
    kind              TEXT NOT NULL,
    default_server_id TEXT NOT NULL REFERENCES servers(id),
    dir               TEXT,
    revision          BIGINT NOT NULL,
    deployed_revision BIGINT NOT NULL,
    env_sealed        TEXT,
    env_keys          TEXT NOT NULL,
    created_by        TEXT NOT NULL,
    created_at        BIGINT NOT NULL,
    updated_at        BIGINT NOT NULL
);
CREATE UNIQUE INDEX stacks_environment_name ON stacks (environment_id, name);

CREATE TABLE stack_revisions (
    stack_id   TEXT NOT NULL REFERENCES stacks(id),
    revision   BIGINT NOT NULL,
    content    TEXT NOT NULL,
    sha256     TEXT NOT NULL,
    note       TEXT,
    author     TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (stack_id, revision)
);

CREATE TABLE stack_nodes (
    stack_id    TEXT NOT NULL REFERENCES stacks(id),
    server_id   TEXT NOT NULL REFERENCES servers(id),
    services    TEXT NOT NULL,
    selected    TEXT NOT NULL,
    revision    BIGINT NOT NULL,
    status      TEXT NOT NULL,
    state       TEXT NOT NULL,
    last_job_id TEXT,
    last_error  TEXT,
    updated_at  BIGINT NOT NULL,
    PRIMARY KEY (stack_id, server_id)
);
CREATE INDEX stack_nodes_server ON stack_nodes (server_id);


-- ======================================================================
-- AI connectors (MCP)
-- ======================================================================

CREATE TABLE oauth_clients (
    client_id     TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    redirect_uris TEXT NOT NULL,
    created_at    BIGINT NOT NULL,
    created_ip    TEXT,
    revoked_at    BIGINT,
    last_used_at  BIGINT
);

CREATE TABLE agent_connections (
    id            TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL REFERENCES users(id),
    client_id     TEXT NOT NULL,
    client_name   TEXT NOT NULL,
    redirect_host TEXT NOT NULL,
    resource      TEXT NOT NULL,
    mode          TEXT NOT NULL,
    created_at    BIGINT NOT NULL,
    last_used_at  BIGINT,
    revoked_at    BIGINT
);
CREATE INDEX agent_connections_user ON agent_connections (user_id);


-- ======================================================================
-- Approvals
-- ======================================================================

CREATE TABLE approvals (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    user_id       TEXT NOT NULL REFERENCES users(id),
    connection_id TEXT,
    requester     TEXT NOT NULL,
    action        TEXT NOT NULL,
    request       TEXT NOT NULL,
    summary       TEXT NOT NULL,
    mfa_required  BIGINT NOT NULL DEFAULT 0,
    status        TEXT NOT NULL,
    created_at    BIGINT NOT NULL,
    expires_at    BIGINT NOT NULL,
    decided_by    TEXT,
    decided_at    BIGINT,
    result        TEXT,
    error         TEXT
);
CREATE INDEX approvals_user ON approvals (user_id, status);
CREATE INDEX approvals_connection ON approvals (connection_id, status);


-- ======================================================================
-- Idempotency keys
-- ======================================================================

CREATE TABLE idempotency_keys (
    principal    TEXT NOT NULL,
    idem_key     TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    state        TEXT NOT NULL,
    status       BIGINT,
    content_type TEXT,
    body         TEXT,
    created_at   BIGINT NOT NULL,
    expires_at   BIGINT NOT NULL,
    PRIMARY KEY (principal, idem_key)
);
CREATE INDEX idempotency_keys_expiry ON idempotency_keys (expires_at);


-- ======================================================================
-- Local names
-- ======================================================================

CREATE TABLE node_names (
    server_id    TEXT NOT NULL PRIMARY KEY REFERENCES servers (id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    label        TEXT NOT NULL,
    explicit     BIGINT NOT NULL DEFAULT 1,
    updated_at   BIGINT NOT NULL
);
CREATE UNIQUE INDEX node_names_label ON node_names (workspace_id, label);


-- ======================================================================
-- Health observations
-- ======================================================================

CREATE TABLE observations_latest (
    resource_type TEXT NOT NULL,
    resource_id   TEXT NOT NULL,
    server_id     TEXT NOT NULL,
    subject       TEXT NOT NULL,
    value         TEXT NOT NULL,
    reason        TEXT,
    detail        TEXT,
    source        TEXT NOT NULL,
    executor      TEXT,
    generation    TEXT NOT NULL,
    observed_at   BIGINT NOT NULL,
    received_at   BIGINT NOT NULL,
    fresh_until   BIGINT NOT NULL,
    skewed        BIGINT NOT NULL,
    changed_at    BIGINT NOT NULL,
    -- 1 once the stale sweeper recorded this value going unknown.
    stale_noted   BIGINT NOT NULL,
    PRIMARY KEY (resource_type, resource_id, server_id, subject)
);
CREATE INDEX observations_latest_fresh ON observations_latest (stale_noted, fresh_until);
CREATE INDEX observations_latest_server ON observations_latest (server_id);

CREATE TABLE observation_history (
    resource_type TEXT NOT NULL,
    resource_id   TEXT NOT NULL,
    server_id     TEXT NOT NULL,
    subject       TEXT NOT NULL,
    generation    TEXT NOT NULL,
    observed_at   BIGINT NOT NULL,
    value         TEXT NOT NULL,
    reason        TEXT,
    detail        TEXT,
    source        TEXT NOT NULL,
    executor      TEXT,
    received_at   BIGINT NOT NULL,
    fresh_until   BIGINT NOT NULL,
    skewed        BIGINT NOT NULL,
    PRIMARY KEY (resource_type, resource_id, server_id, subject, generation, observed_at)
);
CREATE INDEX observation_history_received ON observation_history (received_at);

CREATE TABLE observation_transitions (
    seq           BIGINT NOT NULL PRIMARY KEY,
    resource_type TEXT NOT NULL,
    resource_id   TEXT NOT NULL,
    server_id     TEXT NOT NULL,
    subject       TEXT NOT NULL,
    from_value    TEXT,
    to_value      TEXT NOT NULL,
    reason        TEXT,
    generation    TEXT NOT NULL,
    at            BIGINT NOT NULL,
    evidence_at   BIGINT NOT NULL,
    dedupe        TEXT NOT NULL UNIQUE
);


-- ======================================================================
-- Access profiles and service accounts
-- ======================================================================

CREATE TABLE permission_profiles (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    builtin     BIGINT NOT NULL DEFAULT 0,
    revision    BIGINT NOT NULL,
    archived_at BIGINT,
    created_by  TEXT NOT NULL,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL
);
CREATE UNIQUE INDEX permission_profiles_name ON permission_profiles (name);

CREATE TABLE permission_profile_revisions (
    profile_id TEXT NOT NULL REFERENCES permission_profiles (id),
    revision   BIGINT NOT NULL,
    -- JSON array of Cedar action names, sorted, each a known action.
    actions    TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (profile_id, revision)
);

CREATE TABLE profile_assignments (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users (id),
    profile_id TEXT NOT NULL REFERENCES permission_profiles (id),
    -- `<kind>:<id>`: workspace, cluster, environment, server or service.
    scope      TEXT NOT NULL,
    expires_at BIGINT,
    created_by TEXT NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE INDEX profile_assignments_user ON profile_assignments (user_id);
CREATE INDEX profile_assignments_profile ON profile_assignments (profile_id);

CREATE TABLE service_accounts (
    user_id     TEXT PRIMARY KEY REFERENCES users (id),
    owner_id    TEXT NOT NULL REFERENCES users (id),
    description TEXT NOT NULL DEFAULT '',
    created_by  TEXT NOT NULL,
    created_at  BIGINT NOT NULL
);
CREATE INDEX service_accounts_owner ON service_accounts (owner_id);


-- ======================================================================
-- Alerts
-- ======================================================================

CREATE TABLE alert_rules (
    id           TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    name         TEXT NOT NULL,
    description  TEXT NOT NULL,
    enabled      BIGINT NOT NULL,
    severity     TEXT NOT NULL,
    scope        TEXT NOT NULL,
    subject      TEXT NOT NULL,
    spec         TEXT NOT NULL,
    revision     BIGINT NOT NULL,
    created_by   TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL
);
CREATE INDEX alert_rules_workspace ON alert_rules (workspace_id);

CREATE TABLE alarms (
    id              TEXT NOT NULL PRIMARY KEY,
    workspace_id    TEXT NOT NULL,
    rule_id         TEXT NOT NULL,
    rule_revision   BIGINT NOT NULL,
    title           TEXT NOT NULL,
    severity        TEXT NOT NULL,
    fingerprint     TEXT NOT NULL,
    resource_kind   TEXT NOT NULL,
    resource_id     TEXT NOT NULL,
    server_id       TEXT,
    subject         TEXT NOT NULL,
    state           TEXT NOT NULL,
    fired_value     TEXT NOT NULL,
    fired_at        BIGINT NOT NULL,
    resolved_at     BIGINT,
    end_reason      TEXT,
    acknowledged_by TEXT,
    acknowledged_at BIGINT,
    previous_id     TEXT
);
CREATE UNIQUE INDEX alarms_one_active ON alarms (fingerprint) WHERE state = 'firing';
CREATE INDEX alarms_workspace ON alarms (workspace_id, fired_at);
CREATE INDEX alarms_fingerprint ON alarms (fingerprint, fired_at);

CREATE TABLE alarm_events (
    alarm_id TEXT NOT NULL,
    at       BIGINT NOT NULL,
    kind     TEXT NOT NULL,
    actor    TEXT,
    detail   TEXT NOT NULL
);
CREATE INDEX alarm_events_alarm ON alarm_events (alarm_id, at);

CREATE TABLE alert_trackers (
    fingerprint   TEXT NOT NULL PRIMARY KEY,
    rule_id       TEXT NOT NULL,
    rule_revision BIGINT NOT NULL,
    state         TEXT NOT NULL,
    updated_at    BIGINT NOT NULL
);
CREATE INDEX alert_trackers_rule ON alert_trackers (rule_id);

CREATE TABLE alert_cursors (
    name TEXT NOT NULL PRIMARY KEY,
    seq  BIGINT NOT NULL
);

-- Service-scoped port tunnels replace WireGuard remote access
-- (DECISIONS.md D-100). Portable SQL: TEXT/BIGINT, epoch ms, flags as 0/1.

-- Explicit tunnel grants: an administrator lets one person tunnel to one
-- target (app, stack service, package or a node's SSH) without a role on the
-- service's environment. `ports` is a comma-separated list, or '*' for every
-- port of the target. `expires_at` is 253402300799999 for "never".
CREATE TABLE tunnel_grants (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    user_id      TEXT NOT NULL REFERENCES users(id),
    target_kind  TEXT NOT NULL,
    target_ref   TEXT NOT NULL,
    server_id    TEXT,
    ports        TEXT NOT NULL,
    created_by   TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    expires_at   BIGINT NOT NULL,
    revoked_at   BIGINT,
    note         TEXT NOT NULL
);
CREATE INDEX tunnel_grants_user ON tunnel_grants (user_id);

-- Every tunnel ever opened: history, the console list and restart recovery
-- (open rows are closed as `server_restart` when the server starts, since
-- the sockets live in memory). `grant_id` names the grant that allowed it
-- when no role did.
CREATE TABLE tunnels (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    user_id      TEXT NOT NULL REFERENCES users(id),
    server_id    TEXT NOT NULL REFERENCES servers(id),
    target_kind  TEXT NOT NULL,
    target_ref   TEXT NOT NULL,
    label        TEXT NOT NULL,
    port         BIGINT NOT NULL,
    dial_ip      TEXT NOT NULL,
    bind         TEXT NOT NULL,
    grant_id     TEXT,
    opened_at    BIGINT NOT NULL,
    expires_at   BIGINT NOT NULL,
    closed_at    BIGINT,
    close_reason TEXT,
    bytes_up     BIGINT NOT NULL DEFAULT 0,
    bytes_down   BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX tunnels_user ON tunnels (user_id, opened_at);
CREATE INDEX tunnels_open ON tunnels (closed_at);


-- Services group environments (D-102). A project is the product
-- ("shop"); each environment may belong to one. Environments
-- created before this migration stay unassigned (NULL) until attached.
CREATE TABLE projects (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    name         TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    created_at   BIGINT NOT NULL,
    UNIQUE (workspace_id, name)
);
CREATE INDEX environments_project ON environments (project_id);


CREATE INDEX object_stores_environment ON object_stores (environment_id);


-- Bucket grants (DECISIONS.md D-108): an administrator lets one person work
-- with the objects of one bucket of a store, through Forge, without a role on
-- the store's node, workspace or environment.
-- Portable SQL: TEXT/BIGINT, epoch ms.

-- `access` is 'read' (list, download) or 'readwrite' (also upload, delete).
-- `expires_at` is 253402300799999 for "never", like tunnel_grants.
-- `revoked_at` is the only column that changes after a grant is issued.
-- The rows of a store are deleted with the store.
CREATE TABLE bucket_grants (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    user_id      TEXT NOT NULL REFERENCES users(id),
    store_id     TEXT NOT NULL REFERENCES object_stores(id),
    bucket       TEXT NOT NULL,
    access       TEXT NOT NULL,
    created_by   TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    expires_at   BIGINT NOT NULL,
    revoked_at   BIGINT,
    note         TEXT NOT NULL
);
-- Every request of a grant holder reads that person's grants.
CREATE INDEX bucket_grants_user ON bucket_grants (user_id, expires_at);
-- The store's Access list, newest first.
CREATE INDEX bucket_grants_store ON bucket_grants (store_id, created_at);


-- Alarm notifications (#11, D-110): channels, policies, the alarm
-- transition outbox and deliveries with their attempts.

CREATE TABLE notification_channels (
    id           TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL,
    enabled      BIGINT NOT NULL,
    -- scheme://host[:port] for display; the whole URL is sealed.
    target       TEXT NOT NULL,
    sealed_url   TEXT NOT NULL,
    secret_item  TEXT,
    created_by   TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL,
    email TEXT
);
CREATE INDEX notification_channels_workspace ON notification_channels (workspace_id);

CREATE TABLE notification_policies (
    id                   TEXT NOT NULL PRIMARY KEY,
    workspace_id         TEXT NOT NULL,
    name                 TEXT NOT NULL,
    enabled              BIGINT NOT NULL,
    min_severity         TEXT NOT NULL,
    -- JSON: rule ids, scope (or null), events, channel ids.
    rules                TEXT NOT NULL,
    scope                TEXT NOT NULL,
    events               TEXT NOT NULL,
    repeat_interval_secs BIGINT,
    channels             TEXT NOT NULL,
    created_by           TEXT NOT NULL,
    created_at           BIGINT NOT NULL,
    updated_at           BIGINT NOT NULL,
    email_recipients TEXT
);
CREATE INDEX notification_policies_workspace ON notification_policies (workspace_id);

-- The outbox: one row per alarm transition, written in the transaction that
-- changes the alarm. An alarm fires once and closes once.
CREATE TABLE alarm_transitions (
    alarm_id     TEXT NOT NULL,
    kind         TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    at           BIGINT NOT NULL,
    planned_at   BIGINT,
    -- Planning attempts that failed, and why the last one did. A transition
    -- that keeps failing is marked planned with no deliveries.
    plan_failures BIGINT NOT NULL DEFAULT 0,
    plan_error    TEXT,
    PRIMARY KEY (alarm_id, kind)
);
CREATE INDEX alarm_transitions_unplanned ON alarm_transitions (planned_at, at);

CREATE TABLE notification_deliveries (
    id               TEXT NOT NULL PRIMARY KEY,
    workspace_id     TEXT NOT NULL,
    alarm_id         TEXT,
    event            TEXT NOT NULL,
    occurrence       BIGINT NOT NULL,
    policy_id        TEXT,
    channel_id       TEXT NOT NULL,
    dedupe_key       TEXT NOT NULL,
    status           TEXT NOT NULL,
    attempts         BIGINT NOT NULL,
    next_attempt_at  BIGINT NOT NULL,
    lease_until      BIGINT,
    last_error       TEXT,
    last_http_status BIGINT,
    detail           TEXT,
    created_at       BIGINT NOT NULL,
    finished_at      BIGINT,
    channel_name     TEXT NOT NULL,
    policy_name      TEXT NOT NULL,
    alarm_title      TEXT NOT NULL,
    resource_kind    TEXT,
    resource_id      TEXT,
    silence_id TEXT NULL,
    after_silence BIGINT NOT NULL DEFAULT 0,
    released_at BIGINT NULL,
    recipients TEXT
);
CREATE UNIQUE INDEX notification_deliveries_dedupe ON notification_deliveries (dedupe_key);
CREATE INDEX notification_deliveries_due ON notification_deliveries (status, next_attempt_at);
CREATE INDEX notification_deliveries_workspace ON notification_deliveries (workspace_id, created_at);
CREATE INDEX notification_deliveries_alarm ON notification_deliveries (alarm_id, channel_id);

CREATE TABLE notification_attempts (
    delivery_id TEXT NOT NULL,
    number      BIGINT NOT NULL,
    at          BIGINT NOT NULL,
    ok          BIGINT NOT NULL,
    http_status BIGINT,
    error       TEXT,
    duration_ms BIGINT NOT NULL,
    PRIMARY KEY (delivery_id, number)
);


-- Who last saved a service's health policy while it referred to vault
-- items (issue #12, D-111). A node asks for the header values of such a
-- check just before it probes; Forge answers on this person's authority and
-- checks their roles again each time. One row per service; removed with
-- the service, and when a saved policy has no vault reference.
CREATE TABLE app_health_authority (
    service_id   TEXT PRIMARY KEY,
    saved_by TEXT NOT NULL,
    saved_at BIGINT NOT NULL,
    -- The revision of the policy that person saved; the authority holds for
    -- that policy only.
    revision BIGINT NOT NULL
);


-- Silences and maintenance windows (#11, D-112): a time-bounded suppression
-- of alarm notifications. Evaluation and alarm history are not touched.

CREATE TABLE silences (
    id           TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    -- JSON: {"kind": "workspace" | "cluster" | "environment" | "servers" | "services", ...}.
    scope        TEXT NOT NULL,
    -- JSON: rule ids (empty: every rule).
    rules        TEXT NOT NULL,
    min_severity TEXT,
    max_severity TEXT,
    starts_at    BIGINT NOT NULL,
    -- Exclusive. Ending a silence early moves this to the time it was ended.
    ends_at      BIGINT NOT NULL,
    reason       TEXT NOT NULL,
    created_by   TEXT NOT NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL,
    ended_by     TEXT
);
CREATE INDEX silences_workspace ON silences (workspace_id, ends_at);
CREATE INDEX notification_deliveries_suppressed ON notification_deliveries (status, released_at);


-- User groups for fine-grained authorization (#13). A user can be in many
-- groups; an access profile can be assigned to many groups at any scope.
-- Group assignments live in their own table so `profile_assignments`
-- (one user each) keeps its released shape.

CREATE TABLE access_groups (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    -- Lower-cased name: names are unique regardless of case.
    name_key    TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    created_by  TEXT NOT NULL,
    created_at  BIGINT NOT NULL
);
CREATE UNIQUE INDEX access_groups_name ON access_groups (name_key);

CREATE TABLE access_group_members (
    group_id   TEXT NOT NULL REFERENCES access_groups (id),
    user_id    TEXT NOT NULL REFERENCES users (id),
    created_at BIGINT NOT NULL,
    PRIMARY KEY (group_id, user_id)
);
CREATE INDEX access_group_members_user ON access_group_members (user_id);

CREATE TABLE group_assignments (
    id         TEXT PRIMARY KEY,
    group_id   TEXT NOT NULL REFERENCES access_groups (id),
    profile_id TEXT NOT NULL REFERENCES permission_profiles (id),
    -- `<kind>:<id>`: workspace, cluster, project, environment, server or service.
    scope      TEXT NOT NULL,
    expires_at BIGINT,
    created_by TEXT NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE INDEX group_assignments_group ON group_assignments (group_id);
CREATE INDEX group_assignments_profile ON group_assignments (profile_id);


-- Docker Engine API over mutual TLS (D-125). One row per node whose dockerd
-- listens on TCP with Forge-issued certificates. Each node has its own CA:
-- `ca_cert` is its PEM, `ca_key` the CA key sealed with the master key. The
-- CA signs dockerd's server certificate and the control plane's short-lived
-- client certificates, so a certificate for one node opens no other.
-- `state`: pending | enabled | failed.

CREATE TABLE docker_endpoints (
    server_id TEXT PRIMARY KEY,
    host TEXT NOT NULL,
    port BIGINT NOT NULL,
    state TEXT NOT NULL,
    ca_cert TEXT NOT NULL,
    ca_key TEXT NOT NULL,
    daemon_cert_not_after BIGINT NOT NULL,
    engine_version TEXT,
    api_version TEXT,
    last_error TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);


-- Releases and deployments of services (#73).
--
-- releases: immutable snapshots of a service (spec, env with sealed secrets,
-- artifact, rollout policy), numbered per service.
-- deployments: one rollout of a release; `plan` is the planner's steps and
-- `step` how many are done, so a resumed job continues where it stopped.
-- deployment_locks: one row per service with an active deployment; the
-- primary key is the lock (inserted with the deployment, deleted when it ends).
-- deployment_events: what happened, in order.
-- deploy_slots: the copies of a service on each node ('' = the unslotted
-- instance, 'a'/'b' = side-by-side slots) and their traffic weight.
-- deploy_ports: host ports given to a service's slots, unique across services.

CREATE TABLE releases (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    number BIGINT NOT NULL,
    spec TEXT NOT NULL,
    env TEXT NOT NULL,
    artifact TEXT NOT NULL,
    policy TEXT NOT NULL,
    source TEXT,
    source_sha256 TEXT,
    description TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE (service_id, number)
);

CREATE TABLE deployments (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    release_id TEXT NOT NULL,
    release_number BIGINT NOT NULL,
    from_release BIGINT NOT NULL,
    kind TEXT NOT NULL,
    strategy TEXT NOT NULL,
    state TEXT NOT NULL,
    request TEXT,
    job_id TEXT,
    plan TEXT NOT NULL,
    step BIGINT NOT NULL,
    error TEXT,
    created_by TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    finished_at BIGINT
);

CREATE INDEX deployments_app ON deployments (service_id, created_at);

CREATE TABLE deployment_locks (
    service_id TEXT PRIMARY KEY,
    deployment_id TEXT NOT NULL
);

CREATE TABLE deployment_events (
    deployment_id TEXT NOT NULL,
    seq BIGINT NOT NULL,
    at BIGINT NOT NULL,
    level TEXT NOT NULL,
    server_id TEXT,
    message TEXT NOT NULL,
    PRIMARY KEY (deployment_id, seq)
);

CREATE TABLE deploy_slots (
    service_id TEXT NOT NULL,
    server_id TEXT NOT NULL,
    slot TEXT NOT NULL,
    release_number BIGINT NOT NULL,
    weight BIGINT NOT NULL,
    state TEXT NOT NULL,
    ports TEXT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (service_id, server_id, slot)
);

CREATE TABLE deploy_ports (
    service_id TEXT NOT NULL,
    slot TEXT NOT NULL,
    idx BIGINT NOT NULL,
    port BIGINT NOT NULL UNIQUE,
    PRIMARY KEY (service_id, slot, idx)
);


-- Service log ingest keys (D-128). Each key lets an app send its logs
-- (Seq `X-Seq-ApiKey`) for one service. Only the SHA-256 of the token
-- is stored; `prefix` is its first characters, to tell keys apart.

CREATE TABLE log_ingest_keys (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL,
    name TEXT NOT NULL,
    prefix TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    created_by TEXT,
    created_at BIGINT NOT NULL,
    last_used_at BIGINT
);

CREATE INDEX log_ingest_keys_service ON log_ingest_keys (service_id);

-- Per-user pins shown in the console sidebar: projects, services, nodes and
-- clusters. Rows are left in place when the entity goes away; the API hides
-- them at read time. Deleting the user deletes their pins.
CREATE TABLE user_pins (
    user_id    TEXT   NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind       TEXT   NOT NULL,
    entity_id  TEXT   NOT NULL,
    position   BIGINT NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, kind, entity_id)
);
CREATE INDEX user_pins_by_position ON user_pins (user_id, position);
