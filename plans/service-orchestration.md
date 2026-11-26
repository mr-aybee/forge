# Service orchestration in native mode

Status: plan, not started. Decision record to add when work begins: D-176.

## Goal

Forge is a control plane for nodes, clusters and services. In native mode
(projects / namespaces / services, no Docker Swarm) a Service is the one
managed unit. Forge server and agents orchestrate its container or native
daemon on each node, from the console, the CLI or a YAML file.

Forge has two modes. In Swarm mode Docker Swarm orchestrates. In native mode
Forge orchestrates, for both container Services and native Services (a
daemon installed from a package, an artifact or a command). The target for
native mode is everything Swarm does for a service, in the same YAML shape,
plus what Swarm cannot do: native daemons, richer rollouts, approval,
authorization and audit on every step.

Forge does not integrate with the products it runs. It does not manage their
buckets, users, databases or admin APIs. A product's own console and its
shell are how an operator administers it.

## Decisions taken

| # | Decision |
| --- | --- |
| 1 | One container (or one native daemon) per Service per node. A Service may run on several nodes. |
| 2 | The same image may back any number of Services. Each has its own configuration, ports and data. |
| 3 | Install in the package store is the one path, and its result is a Service. There is no separate "Add". |
| 4 | Any public image (Docker Hub, GHCR) or native artifact can become a Service without a store entry. This works today through New Service and stays. |
| 5 | No Forge-built admin panels for any product. The Object Storage view is removed and is not planned to return. |
| 6 | A Service may declare a console port. The console shows "Open console", a link to the product's own web UI. Sign-in is the product's business. |
| 7 | "Open shell" on a Service opens a shell in its container or on its node (for example to run `redis-cli`). |
| 8 | Bucket attachment (D-106) is removed with the Object Storage view. Credentials reach a consuming Service as ordinary secret env. |
| 9 | YAML uses the Compose / Swarm stack file format. Forge parses it and runs it itself. Keys Forge cannot honour are rejected, never ignored. |
| 9a | Native mode aims at parity with Swarm services (table below). Everything beyond Swarm goes under `x-forge`, so a file stays a valid Compose file and a plain Swarm stack file applies unchanged. |
| 10 | On node failure, a Service with no local data moves to another eligible node. A Service with local data raises an alert and stays. |

## Where the code stands

- `services.spec` is untyped JSON. The typed `AppSpec`
  (`forge-catalog/src/apps.rs`) is built at dispatch.
- `AppSpec` covers image, command, entrypoint, ports, volumes, env, env
  files, CPU, memory, user, hostname, labels, extra hosts, health check,
  stop timeout, read-only root and log size. Restart is a boolean.
- Missing from `AppSpec`: restart policy with attempts, capabilities,
  ulimits, tmpfs, DNS, sysctls, devices, init, extra networks.
- `service_instances` is keyed (service, node), so decision 1 already
  matches the schema.
- `forge.deploy.yaml` places a Service on several nodes and rolls it
  (recreate, rolling, blue-green, canary, manual).
- Nothing reschedules or repairs a Service outside a deployment. The agent
  acts only on signed commands and otherwise relies on Docker's restart
  policy or systemd `Restart=`.
- `forge.stack.yaml` wraps a Compose file that `docker compose` runs on the
  node. Forge reads only the hostname constraint under `deploy:`.
- Store packages are `installations` rows driven by recipe scripts. They are
  not Services.
- `services` has no version column, so two editors overwrite each other.
- The object store is a separate subsystem: `objstore/`, `jobs/storage.rs`,
  `api/storage.rs`, `api/objstore.rs`, `app_storage.rs`, four tables, the
  `objstore-rustfs` recipes, four console pages, CLI commands and docs.

## Swarm parity in native mode

| Swarm gives | Native mode | Step |
| --- | --- | --- |
| Service spec (image, env, mounts, limits, health, caps, …) | Partly there; completed | 1 |
| `replicas`, `mode: global` | Number of nodes, or every eligible node. One copy per node, which is Swarm's `max_replicas_per_node: 1` (decision 1) | 3 |
| Placement constraints and spread | Named nodes and node tags exist; `node.labels` constraints and spread by tag added | 3 |
| Rolling update, rollback | Exists, and goes further (blue-green, canary, manual) | 3 maps the keys |
| `scale` | `forge services scale <name> <n>`: changes the node count and deploys | 3 |
| Restart policy | Completed | 1 |
| Reschedule on node failure, restart failed tasks | Reconciler | 2 |
| Node drain | `forge nodes drain`: the reconciler moves movable Services off and stops placing there | 2 |
| Secrets and configs as files | Vault items mounted read-only at `/run/secrets/<name>`; configs as files at the given target | 3a |
| Overlay network, service names | Container network and cluster DNS already shipped (D-172 to D-175) | done |
| Routing mesh (port open on every node) | Not copied. An ingress endpoint or the cluster DNS name reaches the Service | none |
| Resource reservations | Checked against node capacity at placement | 3 |
| Service logs, tasks | Exist | done |

Beyond Swarm, under `x-forge` on a service:

| Key | Meaning |
| --- | --- |
| `runtime: systemd` with `artifact`, `listen_port` | A native Service in the same file as container Services |
| `strategy: blue-green \| canary \| manual` with its steps | Rollouts Swarm does not have |
| `health` | Startup, readiness and liveness policy run by the node |
| `on_node_failure: move \| alert` | Per-Service failure behaviour |
| `console`, `ingress` | Console link; ingress endpoint and DNS name |
| `nodes.<name>` | Per-node overrides of env, ports and mounts |

Also beyond Swarm: `plan` shows a diff before `apply`, every step is
authorized and audited, and drift is detected and shown.

## Step 1: a complete, typed container spec

One PR. No behaviour change for existing Services.

- Add the missing fields to `AppSpec` as typed values: `restart`
  (`no | always | unless-stopped | on-failure` with `max_attempts`),
  `cap_add`, `cap_drop`, `ulimits`, `tmpfs`, `dns`, `sysctls`, `devices`,
  `init`, `networks`. Keep reading the old boolean `restart`.
- Each new field passes the same policy checks the stack path applies
  (`stacks::check`: privileged, socket binds, host paths).
- The agent maps each field onto the Docker Engine create call in
  `forge-agent/src/apps.rs`.
- Store `services.spec` through one typed struct on read and write.
- Migration `0005`: `services.version BIGINT NOT NULL DEFAULT 0`. Update
  takes the version the editor loaded and fails with 409 "changed since you
  opened it" when it differs.
- Console: the Service editor gets every field, grouped (image, runtime,
  network, storage, limits, security, health).
- CLI: `forge services create|update` flags for the common fields, `--file`
  for the rest.

## Step 2: keep Services in their desired state

One PR. This is the node-level behaviour.

- New service type `ServiceReconciler` outside `api/`, taking ports and the
  `Clock`. It runs on a tick and skips a Service that has an active
  deployment lock.
- The agent already reports container state. The reconciler compares it
  with the desired state of each instance:
  - container missing or exited while the Service is meant to run: submit
    the existing `app.action` start or deploy job;
  - spec drift (the running container does not match the stored release):
    mark the instance `drifted` and show it. Do not redeploy on its own.
- Node offline longer than a grace period (setting, default 5 minutes):
  - Service with no volumes and no host paths: place it on another eligible
    node with the existing `place()` rules and deploy there. Remove the old
    instance when the node returns.
  - Service with local data: alert through `forge-alerts`, do nothing else.
  - A per-Service `on_node_failure: move | alert` overrides the default.
- Node drain: `forge nodes drain <node>` and a console action. The
  reconciler moves movable Services off the node, alerts for the ones with
  local data, and placement skips the node until it is undrained.
- Every reconciler action is a job, authorized as the system principal and
  audited. Steps are idempotent.
- Tests: kill a container and poll for restart; stop an agent and poll for
  the move; confirm a Service with a volume is not moved.

## Step 3: Compose / Swarm format YAML run by Forge

One PR.

- `forge services apply -f <file> --environment <env>` and the same from the
  console (a YAML tab on New Service and on the Service editor).
- Each entry under `services:` becomes one native Service. No
  `docker compose` and no Swarm are involved.
- Mapping:

  | File key | Forge |
  | --- | --- |
  | `image`, `command`, `entrypoint`, `environment`, `env_file`, `ports`, `volumes`, `healthcheck`, `user`, `hostname`, `labels`, `extra_hosts`, `cap_add`, `cap_drop`, `ulimits`, `tmpfs`, `dns`, `sysctls`, `init`, `read_only`, `stop_grace_period` | the matching `AppSpec` field |
  | `deploy.replicas` | number of nodes |
  | `deploy.mode: global` | every eligible node |
  | `deploy.placement.constraints` | `node.hostname` to named nodes, `node.labels.x` to node tags |
  | `deploy.resources.limits` | `cpu_millis`, `memory_mb` |
  | `deploy.restart_policy` | `restart` and `max_attempts` |
  | `deploy.update_config` | rollout: `parallelism` to batch size, `order` to start-first or stop-first, `failure_action: rollback` to auto rollback |
  | `depends_on` | `dependencies` |

  | `deploy.placement.preferences` (`spread`) | spread across the values of a node tag |
  | `deploy.placement.max_replicas_per_node` | accepted only as `1` |
  | `deploy.resources.reservations` | checked against node capacity at placement |
  | `deploy.rollback_config` | rollback batch size and order |
  | `secrets`, `configs` | see step 3a |
  | `x-forge` | the extensions in the parity section |

- Rejected with a message naming the key: `build`, `deploy.endpoint_mode`,
  `privileged`, `network_mode: host` unless policy allows it, and any key
  not in the table.
- `forge services plan -f <file>` prints what would be created, changed,
  moved and removed. `apply` runs it.
- `forge services scale <name> <n>` changes the node count.
- `forge services export <name>` writes the same format back, so the
  console, CLI and file describe one thing.
- `forge.stack.yaml` and `forge.deploy.yaml` stay as they are.

## Step 3a: secrets and configs as files

One PR, after step 3.

- Top-level `secrets:` and `configs:` must be `external: true` and name a
  vault item or a stored config, as on the Swarm path (D-171). `file:`
  entries are refused.
- The server unseals the value inside the deploy step and sends it only in
  the signed command. The agent writes it under a root-only directory per
  Service (mode 0400) and bind-mounts it read-only at `/run/secrets/<name>`
  or the config's `target`.
- A native Service gets the same files at a path given in its env.
- Rotating the vault item marks the Service as needing a redeploy.
- Nothing is logged and nothing reaches the browser.

## Step 4: package store installs become Services

One PR.

- `forge_catalog::Package` gains an optional container template: image
  repository, default ports, default volumes, default env, and
  `console_port`.
- The Install form asks for namespace, node or nodes, and runtime
  (container when the package has a template, native when it has a recipe).
- Container install: creates a normal `container` Service from the template
  and deploys it through the usual path.
- Native install: runs the recipe as today and registers a Service row with
  runtime `package`, as the stack path does for `compose-service`. Start,
  stop, restart, status and upgrade on that Service call the recipe actions.
  Migration adds `installations.service_id`.
- `services.source` records the package id and version the Service came
  from, so the store can show "installed" and offer upgrades.
- Existing installations get a Service row by a one-time backfill.
- The Installed page becomes a filter on Services.

## Step 5: console link and shell

Small PR, can ship with step 4.

- `AppSpec.console { port, path, scheme }`, optional.
- Service page: "Open console" opens `scheme://<address>:<port><path>` in a
  new tab. The address is the Service's ingress endpoint when one exists,
  otherwise the node address. No proxy and no sign-in by Forge.
- Service page: "Open shell" uses the existing container exec for a
  container Service and the node terminal for a native one. Both are
  already authorized and audited.

## Step 6: remove the Object Storage subsystem

One PR, after steps 4 and 5 so existing stores have somewhere to go.

- Generic "adopt a running container as a Service": read the container's
  configuration into an `AppSpec`, keep its container name
  (`services.adopted_from`), and do not recreate it until the first deploy.
  This replaces store discovery and adoption.
- Upgrade path for an existing store: its container is adopted as a Service
  with the same image, env, ports and data mounts, and `console.port` set
  from the store's console port. The container is not restarted.
- RustFS stays in the package store as an ordinary package with a container
  template.
- Remove: `objstore/`, `jobs/storage.rs`, `api/storage.rs`,
  `api/objstore.rs`, `app_storage.rs`, `bucket_grants.rs`,
  `storage_settings.rs`, `forge-catalog/src/storage.rs`, the
  `objstore-rustfs` recipes, the Storage console pages and components, the
  CLI storage commands, the Cedar actions and schema entries, and the docs
  pages.
- A new migration drops `object_stores`, `object_store_members`,
  `object_store_settings` and `object_store_keys`. Released migrations are
  not edited.
- Sealed store credentials stay in the vault as plain vault items, so the
  operator can still read the root key.
- Record in `DECISIONS.md` that D-106 and D-170 are withdrawn.

## What is lost, on purpose

- Bucket, access key and grant management in Forge.
- Bucket attachment to a Service.
- The data-path ownership marker and the `require_mount` check that the
  store recipe enforced. Replacement: deleting a Service never deletes
  volumes or host paths, and a Service with host-path mounts is never moved
  by the reconciler.
- Distributed RustFS set up by Forge. An operator can still run it as one
  Service per node with the peer list in env.

## Not in this plan

- Several replicas of one Service on the same node.
- A routing mesh that opens a Service's port on every node.
- Proxying a product console through Forge, or single sign-on into it.
- Service-to-service attachment of any kind.

## To confirm before starting

- That deleting a Service leaves named volumes and host paths in place
  today. Step 6 depends on it.
- Whether native mode already has a node drain or cordon. Step 2 adds one
  only if it does not.
- Which packages can get a container template in step 4, and their official
  images.
- That container exec is reachable from the Service page and not only from
  the Containers page.

## Each PR also carries

Docs under `docs/`, `reference/rest-api.md` for endpoint changes,
`README.md` feature table, `ARCHITECTURE.md` when a component is added or
removed, and the full gate from `CLAUDE.md`.
