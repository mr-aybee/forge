# Container platform: cluster modes, Docker Swarm, Portainer-level management

Forge gains Portainer-level container management with **two cluster modes**:
Default (today's Forge orchestration) and **Docker Swarm**. Swarm mode keeps
the Project → Environment → Application → Service model, plus Forge's
deployment strategies and CI/CD redeploy webhooks.

All UI uses Forge's own console skin: tokens, the components in `ui.tsx`, and
the dense operator layout. Every phase follows `CLAUDE.md`:
- thin handlers and services on ports;
- Cedar on every path;
- audit on every mutation;
- typed ops;
- migrations after `0001`;
- docs in the same change.

## Decisions
- **D-154 Cluster modes and Swarm.** This supersedes D-033.
  - `Cluster.mode` is `default | swarm`.
  - The mode is chosen at creation and cannot be changed afterwards in v1.
    Converting a cluster is a later migration job.
  - Existing clusters migrate as `default`, with no behaviour change.
- **D-155 Redeploy webhooks.**
  - The hook URL is `POST /hooks/<token>`. The token comes from
    `random_token`, and only its `sha256_hex` is stored.
  - An optional HMAC-SHA256 check accepts GitHub (`X-Hub-Signature-256`),
    GitLab (`X-Gitlab-Token`) or `X-Forge-Signature`.
  - Each hook has a rate limit and a 64 KiB body cap.
  - An optional `?tag=` override is checked against a pattern.
  - The audit actor is `webhook:<id>`. The creator's permissions are
    re-checked each time the hook fires.
- **D-156 Swarm deployment strategies.** Swarm's native orchestration is
  reused wherever it fits (the mapping is under Phase 3).
- **New Cedar actions:** `swarm.manage`, `image.manage`, `volume.manage`,
  `network.manage`, `registry.manage`, `webhook.manage`. Each also goes into
  `forge_core::Action`, the schema, the role groups and the policy-matrix
  tests.

## Phase 0: CI hygiene (done in this PR)
- Windows is paused in CI and release. Local gate only. CI runs when the
  owner asks.

## Phase 1: Cluster modes and Swarm node management
- **Model:**
  - `ClusterMode { Default, Swarm }` on `Cluster`, added by migration
    `0002_cluster_mode.sql` (`TEXT`, default `'default'`).
  - `SwarmState` per cluster: swarm id, manager endpoints, and sealed join
    tokens.
  - Per node: role `manager | worker`, availability, and Swarm node id.
- **Creation:** the **New cluster** dialog gets a mode selector (Default /
  Docker Swarm) with a short explanation. The swarm settings are:
  - advertise interface;
  - default address pool;
  - autolock on or off.
- **Onboarding:** adding a node to a Swarm cluster runs the durable job
  `swarm.node.join`, with these steps:
  1. ensure Docker Engine (reuses `container_runtime.rs` auto-install);
  2. init on the first node, which becomes the manager;
  3. fetch the sealed token;
  4. join;
  5. verify;
  6. record.

  Every step is idempotent: it detects existing membership.
- **Agent:** new closed `SwarmOp`s through the root helper over the Engine API:
  - `init`, `join`, `leave`, `update`;
  - `node_inspect`, `node_update` (availability, role, labels).
- **Lifecycle:**
  - promote, demote, drain, pause, activate;
  - labels;
  - remove (drain → leave → `node rm`);
  - rotate join tokens;
  - manager-quorum warnings: an even count, or fewer than 3 managers.
- **UI:**
  - The cluster **Nodes** tab shows the role and availability badges and the
    lifecycle row actions.
  - A cluster **Swarm** overview shows a visualizer of nodes by tasks, quorum
    health and join tokens. Joining needs MFA.

## Phase 2: Project Explorer on Swarm (services)
- The hierarchy stays as it is. On a Swarm cluster, the service form only
  offers container runtimes. System services are hidden and the API rejects
  them with a typed error. Default clusters keep both kinds.
- **A Swarm service spec:**
  - image (registry from Phase 4), env, secrets, configs;
  - ports (ingress or host mode);
  - mounts and volumes, networks (overlay per environment);
  - resources (limits and reservations);
  - healthcheck;
  - replicas or `global` mode;
  - placement constraints and preferences (spread by label), max replicas
    per node;
  - restart policy;
  - update and rollback config.
- **Naming and networks:** a service becomes a Swarm service named
  `<project>-<env>-<service>`, carrying Forge labels. Each environment gets
  its own overlay network.
- **Lifecycle:**
  - create, update, scale, restart (`--force`), rollback, remove;
  - task list with node and state;
  - logs (service logs);
  - shell into a task's container (reuses container exec on the node).
- **Swarm stacks:** compose stacks on Swarm clusters deploy with
  `docker stack deploy --with-registry-auth`, reusing `StackKind` (adds
  `Swarm`), revisions, policy and the stack UI.
- **Secrets and configs:** create and remove. Values are sealed in transit and
  are never shown again.

## Phase 3: Deployment and orchestration
Forge stays the single place to manage deploys. The `deploy.rs` strategy, the
deployment record, its events and its UI apply to both modes.

| Forge strategy | Swarm implementation |
| --- | --- |
| Recreate | `update_config.order=stop-first`, `parallelism=0` (all at once) |
| Rolling | Native: `parallelism` = batch size, `order` start-first/stop-first from `Replacement`, `delay`, `monitor`, `failure_action=rollback`, `max_failure_ratio` |
| Blue-Green | Two services, `<svc>-blue` and `<svc>-green`. The new colour deploys at full replicas; once healthy, Forge's ingress (Sites / Nginx edge) switches the upstream, then the old colour is scaled to 0 and kept for instant rollback |
| Canary | A `<svc>-canary` service scaled to the step weight next to the stable one. Ingress weights are shifted in steps with health gates, then the stable service is promoted and the canary removed |
| Manual | Forge drives a rolling update batch by batch and waits for an approval between batches |
| HA (service property) | Replicas ≥ 2 with spread placement across nodes or zones, `max_replicas_per_node`, restart policy, healthcheck gating, and ≥ 3 managers. A readiness score is shown in the UI |

- Deployment events stream from Swarm task states into the existing deploy
  timeline.
- Rollback uses the native `service rollback` for Rolling/Recreate, and the
  colour switch for Blue-Green and Canary.

## Phase 4: Docker resources and registries (both modes)
- **Per node:** typed wrappers over the existing Engine API proxy. There are
  no new agent ops.
- **A node Docker workspace:**
  - Containers: kill, pause, rename, recreate, a file browser, prune.
  - Images: pull, tag, push, build, import/export, prune.
  - Volumes and Networks: full management.
  - Live Events, Info, and Prune.
- **Registries:** Docker Hub, GHCR, GitLab, ECR and custom. Credentials are
  sealed with `Envelope` and used for pulls, pushes, stack deploys and
  `--with-registry-auth`. The model, port and store come with migration
  `0003_registries.sql`. UI under **Settings → Registries**.

## Phase 5: CI/CD webhooks
- **Targets:** an application service in either mode, a compose or Swarm
  stack, or a container.
- **Tables:**
  - `webhooks`: target, token hash, sealed HMAC secret, tag pattern, enabled,
    last fired;
  - `webhook_deliveries`: a bounded history of deliveries.
- **Flow:**
  1. A public route with no session.
  2. Constant-time token lookup.
  3. HMAC check.
  4. Rate limit.
  5. The hook submits the normal deployment, so the configured strategy
     applies.
  6. The response is `202 { job, status_url }`.

  CI polls `GET /hooks/<token>/jobs/<id>` to wait for the result.
- **UI:** a **Webhooks** tab on each service and stack:
  - create, copy URL, regenerate, enable/disable;
  - delivery history;
  - ready-made snippets for GitHub Actions, GitLab CI and curl.
- **CLI:** `forge webhook create|list|rotate|delete`.

## Phase 6: Stretch
- Compose app templates in the App Store.
- Git-backed stacks with deploy keys and redeploy on push.
- Converting a cluster from Default to Swarm.

## Every phase includes
- Cedar, audit constants defined once, and integration tests (real SQLite, a
  fake Docker Engine, deadline polling).
- MCP tools where stacks already have them.
- CLI commands.
- Docs:
  - concepts: cluster modes, Swarm;
  - guides: create a Swarm cluster, deploy a service, deployment strategies on
    Swarm, registries, webhooks with CI recipes;
  - reference: REST, config, CLI.
- `ARCHITECTURE.md`, the diagram, `README.md` and the roadmap.

## Verification
- Local gate: `fmt`, `clippy`, `test`, `deny`, and the UI, docs and website
  builds.
- On the Linux lab (lab-a…c), check each of these:
  1. Create a Swarm cluster. lab-a initialises as the manager, and lab-b and
     lab-c join as workers.
  2. Promote lab-b to manager, then drain lab-c and confirm its tasks move.
  3. Deploy a 3-replica service with spread placement.
  4. Run Rolling, Blue-Green and Canary deploys, and confirm each rolls back
     cleanly.
  5. Trigger a redeploy with `curl` and with a GitHub-style HMAC, and watch it
     in the deploy timeline.
- Check that existing Default clusters are unchanged.
- CI runs only when the owner asks.
