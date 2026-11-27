---
title: Services file (Compose format)
description: Every key of a Compose / Swarm format services file that Forge runs as native services, with its type, default and Forge mapping.
sidebar:
  order: 10
---

A services file is a Compose / Swarm format YAML file that
[`forge services apply`](/docs/reference/cli/services/#forge-services-apply)
runs as native Forge services: every entry under `services:` becomes one
service. No `docker compose` and no Docker Swarm are involved. The walkthrough
is [Run services from a Compose file](/docs/guides/services-from-compose/).

Forge reads the file strictly. A key that is not on this page is refused
with its dotted path, for example `services.web.build: Forge runs images: …`.
Keys that start with `x-` (other than `x-forge`) are Compose extension
fields and are ignored.

## Top-level keys

| Key | Type | Default | Example | Forge |
| --- | --- | --- | --- | --- |
| `services` | mapping | — (required) | `services: {web: {image: nginx}}` | One Forge service per entry. Names: lowercase letters, digits and dashes, at most 40; at most 100 services |
| `name` | string | `default` | `name: shop` | The file's source. An apply removes services of this source the file no longer lists |
| `version` | any | — | `version: "3.9"` | Ignored |
| `volumes` | mapping | — | `volumes: {data: null}` | Named volumes; each entry empty or `external: true`. Docker creates them on the node |
| `networks` | mapping | — | `networks: {back: {external: true, name: corp-back}}` | Existing Docker networks; each needs `external: true`, `name` optional |
| `secrets`, `configs` | mapping | — | `secrets: {db_password: {external: true, name: "DB password"}}` | Credential vault items. Each entry needs `external: true`; `name` is the vault item's name (default: the key). `file`, `environment` and `content` are refused: the server never reads files. See [Secrets and configs](#secrets-and-configs) |

## Service keys

| Key | Type | Default | Example | Forge |
| --- | --- | --- | --- | --- |
| `image` | string | — (required for containers) | `image: nginx:1.27` | `image` |
| `command` | string or list | image default | `command: ["serve", "--port", "80"]` | `command`, run with `/bin/sh -c`; list words are shell-quoted |
| `entrypoint` | string or list | image default | `entrypoint: /init.sh` | `entrypoint` (words without spaces) |
| `working_dir` | string | image default | `working_dir: /srv` | `working_dir` |
| `environment` | mapping or `KEY=value` list | — | `environment: {MODE: prod}` | Plain variables. Every key needs a value. Secret variables already set on the service stay |
| `env_file` | path or list | — | `env_file: app.env` | Read by the CLI relative to the file and merged into `environment` (`environment` wins). The API refuses it |
| `ports` | list | — | `ports: ["8080:80", "53:53/udp"]` | `[ip:]host:container[/udp]`. Long form: `target`, `published`, `protocol`, `host_ip`, `mode: host`. A port without a host port and ranges are refused |
| `volumes` | list | — | `volumes: ["data:/var/lib/data", "/srv/conf:/etc/conf:ro"]` | `source:/target[:ro]`. Long form: `type: bind\|volume\|tmpfs`, `source`, `target`, `read_only`, `tmpfs.size`. Anonymous and relative sources are refused |
| `healthcheck` | mapping | none | `healthcheck: {test: curl -f localhost, interval: 10s}` | `test` (string, `CMD`, `CMD-SHELL` or `NONE`), `interval` (`30s`), `timeout` (`5s`), `retries` (`3`), `start_period` (`0s`), `disable` |
| `user` | string | image default | `user: "1000:1000"` | `user` |
| `hostname` | string | container id | `hostname: web` | `hostname` |
| `labels` | mapping or `k=v` list | — | `labels: {team: core}` | `labels` (`forge.*` is reserved) |
| `extra_hosts` | list or mapping | — | `extra_hosts: ["db:192.0.2.5"]` | `name:ipv4` entries |
| `cap_add` | list | — | `cap_add: [NET_ADMIN]` | `cap_add`; needs `stack.privileged` |
| `cap_drop` | list | — | `cap_drop: [ALL]` | `cap_drop` |
| `ulimits` | mapping | — | `ulimits: {nofile: {soft: 1024, hard: 4096}}` | `ulimits`; a number sets both limits |
| `tmpfs` | string or list | — | `tmpfs: ["/run", "/tmp:size=64m"]` | `tmpfs`; only the `size=` option |
| `dns` | string or list | node default | `dns: [192.0.2.53]` | `dns` (IP addresses) |
| `sysctls` | mapping or `k=v` list | — | `sysctls: {net.core.somaxconn: 1024}` | `sysctls`; needs `stack.privileged` |
| `devices` | list | — | `devices: ["/dev/fuse:/dev/fuse:rwm"]` | `devices`; needs `stack.privileged` |
| `init` | boolean | `false` | `init: true` | `init` |
| `read_only` | boolean | `false` | `read_only: true` | `read_only` |
| `stop_grace_period` | duration | Docker default | `stop_grace_period: 30s` | `stop_timeout_secs` |
| `networks` | list or mapping | — | `networks: [back]` | Existing networks declared at the top level; `default` is the cluster container network. Network options are refused |
| `restart` | string | `unless-stopped` | `restart: on-failure:3` | `no`, `always`, `unless-stopped`, `on-failure[:n]` |
| `depends_on` | list or mapping | — | `depends_on: [db]` | Services of the same file applied first. Conditions are accepted and only order the apply |
| `deploy` | mapping | — | see below | Placement, limits and rollout |
| `x-forge` | mapping | — | see below | Forge extensions |
| `secrets` | list | — | `secrets: [db_password]` | Vault items declared at the top level, mounted as files. Short form: the key. Long form: `source`, `target`, `mode`, `uid`, `gid` |
| `configs` | list | — | `configs: [{source: app_conf, target: /etc/app/app.conf}]` | As `secrets`, with a different default target and mode |
| `build`, `privileged`, `network_mode`, `container_name`, `links`, `pid`, `ipc` | — | — | — | Refused with the reason |

## Secrets and configs

A secret or config is a credential vault item mounted as a read-only file.
The file and the service store only the item's name; the server unseals the
value inside the deploy job and sends it only in the signed command to the
node. Referencing an item needs the vault's use permission (a platform
administrator with MFA, `secret.read`, not an API key).

| Long-form key | Type | Default | Example | Meaning |
| --- | --- | --- | --- | --- |
| `source` | string | — (required) | `source: db_password` | A key under the top-level `secrets` or `configs` |
| `target` | path | secret: `/run/secrets/<source>`; config: `/<source>` | `target: /etc/app/app.conf` | Path in the container. A relative secret target goes under `/run/secrets`. Its last part is the file name; two files of one service may not share it. Not under `/proc`, `/sys` or `/dev` |
| `mode` | octal | secret `0400`, config `0444` | `mode: 0440` | Permission bits, at most `0777`. Write `0440` or `"0440"` |
| `uid`, `gid` | number or string | `0` | `uid: "1000"` | Owner inside the container. A container that runs as another user needs this or a readable `mode` |

A native (`x-forge.runtime: systemd`) service gets the same files in a
root-owned folder its account can read, named in `FORGE_SECRETS_DIR`.
Values are at most 4096 bytes (the vault's limit). `forge services export`
writes the references back, never a value.

```yaml
services:
  api:
    image: registry.example/api:2
    secrets:
      - db_password
      - source: tls_key
        target: key.pem
        mode: 0440
        uid: "1000"
    configs:
      - source: app_conf
        target: /etc/api/app.conf
secrets:
  db_password: { external: true }
  tls_key: { external: true, name: "API TLS key" }
configs:
  app_conf: { external: true, name: api-config }
```

## `deploy`

| Key | Type | Default | Example | Forge |
| --- | --- | --- | --- | --- |
| `mode` | `replicated` or `global` | `replicated` | `mode: global` | `global` runs one copy on every eligible online node |
| `replicas` | integer, nodes | `1` | `replicas: 3` | Number of nodes, 1–1000, one copy each. Fewer eligible nodes is refused. Not with `global` |
| `placement.constraints` | list | — | `["node.hostname == lab-a", "node.labels.zone == eu"]` | `node.hostname` or `node.id` name nodes (several mean any of them); `node.labels.<key>` requires the node tag `<key>:<value>`. `!=` and other fields are refused |
| `placement.preferences` | list | — | `[{spread: node.labels.rack}]` | Spread the copies over the values of one tag key |
| `placement.max_replicas_per_node` | integer | `1` | `max_replicas_per_node: 1` | Only `1` |
| `resources.limits.cpus` | number | none | `cpus: "0.5"` | `cpu_millis` |
| `resources.limits.memory` | size | none | `memory: 512M` | `memory_mb` |
| `resources.reservations.cpus` | number | none | `cpus: 2` | Nodes that report fewer CPUs are not eligible |
| `resources.reservations.memory` | size | none | `memory: 1G` | Nodes that report less memory are not eligible |
| `restart_policy.condition` | `none`, `on-failure`, `any` | `any` | `condition: on-failure` | `no`, `on-failure`, `always`. Not together with `restart` |
| `restart_policy.max_attempts` | integer | `0` | `max_attempts: 3` | On `on-failure` only |
| `update_config`, `rollback_config` | mapping | — | `{parallelism: 2, order: start-first}` | `parallelism`, `delay`, `order` (`start-first`, `stop-first`), `failure_action` (`rollback`, `pause`, `continue`), `monitor`, `max_failure_ratio`. Stored and exported; an apply deploys changed services on all their nodes at once |
| `endpoint_mode`, `labels`, `restart_policy.delay`, `restart_policy.window` | — | — | — | Refused |

Eligible nodes are in the environment's cluster, Linux, not drained and
match the constraints and reservations. Online nodes come first, and nodes
the service already runs on come before others.

## `x-forge`

| Key | Type | Default | Example | Forge |
| --- | --- | --- | --- | --- |
| `runtime` | `container` or `systemd` | `container` | `runtime: systemd` | A native service: needs `command`, no `image` |
| `listen_port` | integer | — | `listen_port: 9000` | Native service port for the default health check |
| `on_node_failure` | `move` or `alert` | by movability | `on_node_failure: alert` | What the reconciler does when the node is lost |
| `mesh` | boolean | `true` | `mesh: false` | Join the cluster container network |
| `strategy`, `health`, `console`, `ingress`, `nodes`, `artifact` | — | — | — | Refused: use [`forge.deploy.yaml`](/docs/reference/deployment-yaml/) or the console |

## Units

Durations: `500ms`, `30s`, `1m30s`, `2h`, or a number of seconds. Sizes:
`512M`, `1G`, `256MiB`, `1536k`, or a number of bytes, rounded up to MiB.
CPUs: a number such as `0.5`.

## Example

```yaml
name: shop
services:
  db:
    image: postgres:17
    environment:
      POSTGRES_PASSWORD: change-me
    volumes: ["pg:/var/lib/postgresql/data"]
    deploy:
      placement:
        constraints: ["node.hostname == lab-b"]
  api:
    image: registry.example/api:2
    ports: ["8080:8080"]
    depends_on: [db]
    deploy:
      replicas: 2
      placement:
        preferences: [{ spread: node.labels.zone }]
      resources:
        limits: { cpus: "1", memory: 512M }
volumes:
  pg: {}
```
