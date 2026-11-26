# Native mode cluster networking: implementation spec

Status: agreed design, not started. This file is the authority for the three
steps below. Where this file and an agent's memory of "how it should work"
differ, this file wins. Anything not written here is a question for the
owner, not a guess.

Related: `ARCHITECTURE.md` (fabric, local names), `DECISIONS.md` (D-084 hosts
file names), `docs/src/content/docs/guides/cluster-network.md`,
`docs/src/content/docs/concepts/local-names.md`,
`docs/src/content/docs/reference/compose-extensions.md`.

## 0. Vocabulary

| Term | Meaning | Where it exists today |
| --- | --- | --- |
| Fabric | A cluster's WireGuard mesh and its address plan | `forge_catalog::fabric`, `forge-store/src/fabrics.rs`, `api/fabric.rs`, `api/fabric_reconcile.rs` |
| Interface | `forge` + first 6 chars of the cluster id, e.g. `forgea1b2c3` | `fabric::interface_name` |
| Node address | A `/32` lease of kind `node` in the fabric subnet | `LeaseKind::Node` |
| Instance block | A `/24` (or `/28` on a /24 fabric) lease of kind `block` per node | `LeaseKind::Block`, carried in every peer's `allowed_ips` by `plan_mesh` |
| Service VIP | A `/32` lease of kind `service`; reserved, never configured | `LeaseKind::Service`. **Out of scope. Do not configure it.** |
| Environment | The namespace. Belongs to one project, runs in one cluster | `forge_core::model::Environment` |
| Native service | A `Service` with runtime `service` (systemd) or `container` | `forge_core::model::Service`, placed by `ServiceInstance` |
| Stack | A compose stack; its services are registered as `Service` rows with runtime `compose-service` | `forge-server/src/stacks.rs`, `jobs/stack.rs` |
| Stack part | The per-node compose tree the helper receives | `forge_catalog::stacks::StackPart` |
| Names list | The per-node list of `NameEntry {name, ip}` pushed by `names.rs` | `forge-server/src/names.rs`, `HostNames` command |
| Suffix | `local` by default, settable | `names::suffix` |

Out of scope for every step, by owner decision: service VIPs, nftables or
IPVS balancing, ingress upstreams by name, canary weights, firewall policy
between environments, changes to the Deployment host-port slots
(20000–29999), Windows nodes, Swarm mode.

## 1. Step 1: a mesh-routable address for every container

### 1.1 Behaviour

On every Linux member of a cluster whose fabric is applied, the node holds
one Docker bridge network named `forge-<iface>` (e.g. `forge-forgea1b2c3`)
whose subnet is that node's instance block. Every Forge-managed container on
that node is attached to it in addition to its stack networks, unless opted
out. Docker's routed gateway mode is used, so no masquerade and no inbound
filtering for that network. Nothing in the mesh changes: the block is
already routed by every peer.

### 1.2 Catalog: `forge_catalog::fabric`

Add to `FabricOp` (tag `op`, snake_case):

```rust
/// Ensure the node's container network for `iface` exists with exactly
/// this subnet, MTU and routed gateway mode. Recreates it only when the
/// subnet differs (which restarts attached containers). Linux only.
NetworkEnsure { iface: String, subnet: Cidr, mtu: u16 },
/// Remove the node's container network for `iface` if it exists and has
/// no attached containers; with containers attached, returns an error
/// naming them.
NetworkRemove { iface: String },
```

- `validate`: `valid_fabric_iface(iface)`, `subnet.prefix() >= 20 && <= 28`,
  `mtu` in `1280..=1500`.
- New helper `pub fn network_name(iface: &str) -> String` returning
  `format!("forge-{iface}")`. Use it everywhere; never format the name inline.
- `MESH_MTU` (1420) is the MTU passed.
- The result JSON of `NetworkEnsure`: `{ "name", "subnet", "created": bool,
  "recreated": bool, "docker_version": "28.1.0" }`.

### 1.3 Agent helper: `forge-agent/src/fabric.rs` and `docker_ops.rs`

- Dispatch in `run_op`. Both ops are `#[cfg(not(windows))]`; on Windows
  return `Err("container networks need a Linux node")`.
- Docker is reached the way `docker_ops.rs` does (`call` over the Unix socket).
  Do not shell out to `docker network create`.
- **Version gate.** `GET /version`; parse `Version` as semver; refuse when
  `< 28.0.0` with exactly:
  `docker 28.0 or newer is needed for the cluster container network (found <v>)`.
  Return it with `error_code = "unsupported"` (`RefusalCode::Unsupported`,
  which already exists); do not add a new variant.
- **Do not call `subnet_free`** for this network. That check refuses any
  subnet overlapping a node route, and the fabric's `/16` connected route on
  the WireGuard interface always overlaps the block. Instead check only that
  no *other* Docker network uses an overlapping subnet
  (`GET /networks`, IPAM configs), and that the block lies inside the
  interface's address prefix (`ip -j addr show <iface>`).
- Create body (`POST /networks/create`):

```json
{
  "Name": "forge-<iface>",
  "Driver": "bridge",
  "CheckDuplicate": true,
  "Attachable": true,
  "IPAM": { "Config": [ { "Subnet": "<block>" } ] },
  "Options": {
    "com.docker.network.driver.mtu": "1420",
    "com.docker.network.bridge.gateway_mode_ipv4": "routed",
    "com.docker.network.bridge.name": "fg<first 8 chars of iface>"
  },
  "Labels": { "forge.managed": "true", "forge.fabric": "<iface>" }
}
```

- Ensure logic: inspect by name. Missing → create. Present with the same
  subnet and both options → no-op. Present with a different subnet → if
  containers are attached, stop and remove them from the network is **not**
  allowed; return error `network <name> has <n> containers on subnet <old>;
  redeploy stacks to move to <new>` and let the server surface it. With no
  containers → remove and create.
- Verify `net.ipv4.ip_forward` is `1` after create (read
  `/proc/sys/net/ipv4/ip_forward`). If `0`, set it and persist in
  `/etc/sysctl.d/90-forge-fabric.conf` exactly as `apply` does for relays.
- `Remove` of the fabric (existing `FabricOp::Remove`) must also run
  `NetworkRemove` first; with containers attached it fails and the fabric
  removal reports it in `not_cleaned`.

### 1.4 Server reconciler: `api/fabric_reconcile.rs`

- In `converge`, after `FabricOp::Apply` succeeds on a Linux node, run
  `NetworkEnsure { iface, subnet: <node's block lease>, mtu: MESH_MTU }`.
  Its failure is a node failure with the same backoff as other steps.
- The block lease is the `Lease` with `kind == LeaseKind::Block` and
  `owner == server id` in the fabric record. If missing, skip with a
  `tracing::warn!` and mark the node `waiting for a block`, do not error.
- Store the result's `docker_version` in `NodeState` (new optional field,
  `#[serde(default)]`), shown in the Network page's node row.
- Subnet change (`PUT /clusters/{id}/fabric` with a new subnet) already
  re-leases blocks; the next converge sends the new subnet and the helper
  refuses while containers are attached. The API response of the subnet
  change must include `"restarts_containers": true` and the UI dialog must
  show "Stacks on these nodes must be redeployed to move to the new block"
  with the node list.

### 1.5 Compose injection: `forge_catalog::stacks` and the helper

Injection happens **in the helper**, after `StackPart::prepare` and the policy
check and in the same place as `add_log_url`, so what is checked is what
runs. The server decides *whether* and passes the name.

- `StackPart` gains:

```rust
/// Docker name of the node's cluster container network, when every
/// service not opted out must join it. `None`: no injection.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub mesh_network: Option<String>,
```

  `StackPart` has `deny_unknown_fields`, so an older helper refuses a part
  that carries the field. The server sets it only when the node's agent
  version is at least the version that ships this change (compare
  `Server::agent_version` with the constant `MIN_AGENT_MESH_NETWORK` defined
  once in `forge-server/src/stacks.rs`); otherwise it leaves `None` and adds
  the plan finding `Info: "<node>: update its agent to put services on the
  cluster network"`.
- New `pub fn add_mesh_network(tree: &mut Value, docker_name: &str)` next to
  `add_log_url`:
  - For every service: skip when `network_mode` is set to anything, or when
    `x-forge.mesh == false` on the service, or when the stack's top-level
    `x-forge.mesh == false`.
  - `networks` absent → set to `["default", "forge-mesh"]`.
  - `networks` is a list → append `"forge-mesh"` if absent.
  - `networks` is a map → insert key `"forge-mesh": {}` if absent.
  - Top level: insert `networks.forge-mesh = { "external": true, "name": docker_name }`
    if any service was attached. The key `forge-mesh` is reserved: a stack
    that defines a network with that key is refused in `check` with
    `Blocked: "the network key `forge-mesh` is reserved"`.
- `x-forge` keys are read before `x-forge` is stripped from the part; read
  them in `add_mesh_network` via the raw tree the helper already has.
  Document `x-forge.mesh` (service and top level, boolean, default `true`)
  in `compose-extensions.md`.
- Policy: an `external` network is allowed only when its Docker name equals
  the part's `mesh_network`; any other external network keeps today's rule.
  (Check `docker_network_name` in `check`.)
- `dns` / `dns_search` injection belongs to step 2; do not add it here.

### 1.6 Server: deciding `mesh_network`

In `stacks::plan_unredacted`, per node: `mesh_network = Some(network_name(iface))`
when all of: the node's cluster has a fabric whose status for that node is
`in sync` (reuse `fabric_ips`'s source, extend it to return the iface and
sync state), the node is Linux, the agent version is new enough, and the
document's top-level `x-forge.mesh` is not `false`. `NodePlan` gains
`mesh_network: Option<String>` and `preview` shows per service
`"mesh": true|false|"off (x-forge.mesh)"|"off (network_mode)"`.

Preview notes (findings, `Severity::Info`), exact texts:

- cluster without a fabric: `services are reachable from other nodes only
  through published ports; create the cluster network to reach them by name`
- node not in sync: `<node>: cluster network not applied yet; services will
  join it on the next deploy after it is in sync`

### 1.7 Native `container` services

The helper's `docker run` argument builder for `forge-app-<env>-<svc>`
(`forge-agent/src/docker.rs`, the function that builds the create body)
gains the network when the command carries it. The command type for app
deploys gains `mesh_network: Option<String>` with the same serde attributes
and the same version gate. The `Service.spec` JSON gains `"mesh": bool`
(default `true`), editable in the service form. No change for runtime
`service` (systemd).

### 1.8 Reporting container addresses (feed for step 2)

The helper's stack `ps` (`forge-agent/src/stacks.rs::ps`) adds per row:

```json
{ "service": "api", "name": "forge-prod-orders-api-2", "number": 2,
  "state": "running", "health": "healthy", "mesh_ip": "10.64.12.4", "ports": [...] }
```

- `number` from label `com.docker.compose.container-number`.
- `mesh_ip` from `GET /containers/json?all=1&filters={"label":["com.docker.compose.project=<project>"]}`,
  `NetworkSettings.Networks["forge-<iface>"].IPAddress`; absent when not
  attached. `docker compose ps` does not give addresses; do not try.
- The server stores these rows unchanged in the stack node `state` JSON
  (`jobs/stack.rs::record`), so nothing else changes in storage.
- Freshness: the agent sends a `StackState` push (new `Command`-less
  message `StackStates { states_json }` in `forge-proto`) when a Docker event
  of type `container` with a `com.docker.compose.project` label arrives
  (`docker_ops::events` already reads events) and at most every 30 s per
  node while anything changed. The server handles it in `agent_hub/handlers.rs`
  by updating the stack node `state` and calling `names::poke()`.
- Native containers: the existing container list for `forge-app-*`
  containers (`docker.rs`, the view that already reads
  `NetworkSettings/Networks`) adds `mesh_ip` the same way.

### 1.9 UI

- Stack editor: toggle **Cluster network**, bound to top-level `x-forge.mesh`.
- Stack preview: per-service **mesh** column and the two notes above.
- Stack page: badge **Not on the cluster network** with **Redeploy** when the
  node is in sync but the deployed revision's state rows have no `mesh_ip`.
- Cluster Network page: node row shows `docker_version` and the network name.
- Change network dialog: the restart warning (1.4).

### 1.10 Docs and records

- `cluster-network.md`: new section "Container network", Docker 28
  requirement, remove "there is no internal DNS resolver yet" only in step 2.
- `compose-extensions.md`: `x-forge.mesh`, the reserved `forge-mesh` key,
  the external-network rule.
- `compose-stacks.md`: the two-legs model.
- `rest-api.md`: the preview fields, `restarts_containers`.
- `DECISIONS.md`: D-xxx "Container addresses come from the node's instance
  block via a routed Docker bridge; no per-container WireGuard; Docker 28 minimum".
- `ARCHITECTURE.md`: the fabric paragraph and the SVG.

### 1.11 Tests

- `forge-catalog`: `add_mesh_network` for list, map, absent, `network_mode`,
  `x-forge.mesh: false` at both levels, reserved key refused; `FabricOp`
  validation bounds.
- `forge-agent`: version gate parsing (`27.5.1` refused, `28.0.0` ok,
  `28.1.0-rc1` ok); ensure logic on a fake Docker transport (missing, same,
  different subnet with and without containers).
- `forge-server`: plan sets `mesh_network` only when fabric in sync and
  agent new enough; preview findings texts; `it` test that an old agent
  version yields `None` and the Info finding.
- Lab (lab-operator): two stacks on lab-b and lab-c without published ports;
  `curl` container to container; `docker inspect` on lab-c shows the source
  address unchanged; `wg show` counters move.

## 2. Step 2: naming

### 2.1 Records

`names::build_fleet` adds rows. `NameRow` gains `kind` values
`"stack-service"` and `"replica"` and keeps one row per (name, address):
several rows with one name are several A records, which the hosts file and
forge-dns both handle.

| Row kind | Name | `mesh_address` | `address` | Included when |
| --- | --- | --- | --- | --- |
| `stack-service` | `<svc>.<stack>.<env>.<cluster>.<suffix>` | the container's `mesh_ip` | `None` | state `running` and (health `healthy` or health empty) |
| `replica` | `<svc>-<number>.<stack>.<env>.<cluster>.<suffix>` | the container's `mesh_ip` | `None` | state `running`, any health |
| `service` (native, runtime `container`) | `<svc>.<env>.<cluster>.<suffix>` | the container's `mesh_ip` when present, else the node's mesh address | node address (as today) | as today |
| `service` (native, runtime `service`) | unchanged | unchanged | unchanged | unchanged |
| `node` | unchanged | | | |

Rules:

- Labels through `label_from` for service, stack and environment names.
  `label_from` returning `None` drops the row silently; no renaming on the fly
  beyond what `label_from` already does.
- Only stack services with at least one container port in the compose
  document get rows (`ports:` or `expose:`). A service with neither has no
  name. The SRV port is the first container-side port.
- `address: None` for container rows means `entries_for` omits them for nodes
  outside the fabric, which is correct: those nodes cannot route the block.
- Keep the existing rule "a service name that equals a node name: the node
  keeps it". Add: a `replica` name that equals a `stack-service` or `service`
  name is dropped, and the plan preview warns
  `service `api-2` clashes with replica 2 of `api``.
- Service rows: a stack's services are registered as `Service` rows named
  `<stack>-<svc>` (see `jobs/stack.rs::register`), and `api/services.rs`
  refuses a native service whose name exists in the environment. **Keep
  that refusal unchanged.** It is about the row name, not the DNS name: the
  stack row `orders-api` is named `api.orders.prod...` while a native
  `orders-api` would be `orders-api.prod...`. A native service and a stack
  with the same name (`orders` and stack `orders`) do not collide today and
  need no new rule. Add a test that a stack `orders` and a native service
  `orders` in one environment both get their rows.
- SRV rows: a new `NameRow.srv: Option<(u16 /*port*/, String /*proto*/)>`;
  rendered only by forge-dns, never in the hosts file.

### 2.2 Wire: `HostNames` message

Add to `forge_proto::messages::HostNames`:

```rust
/// JSON object `forge_catalog::names::Records` with the full record set
/// for forge-dns (A, SRV, PTR, TTL). Empty from older servers; the agent
/// then writes no `names.json`.
#[prost(string, tag = "4")]
pub records_json: String,
/// The zone forge-dns is authoritative for, e.g. `c1.local`.
#[prost(string, tag = "5")]
pub zone: String,
/// Addresses of the server-host forge-dns, for the second tier; may be empty.
#[prost(string, repeated, tag = "6")]
pub upstream_forge_dns: Vec<String>,
```

Older agents ignore unknown fields; `entries_json` keeps being sent so the
hosts file keeps working everywhere.

### 2.3 `names.json`: the forge-dns feed

Defined once as `forge_catalog::names::Records` (serde, `deny_unknown_fields`
off so forge-dns can read newer files):

```json
{
  "version": 1,
  "generated_at_ms": 1764115200000,
  "zones": ["c1.local"],
  "ttl": 5,
  "records": [
    { "name": "api.orders.prod.c1.local", "type": "A", "value": "10.64.10.2" },
    { "name": "api.orders.prod.c1.local", "type": "A", "value": "10.64.12.4" },
    { "name": "api-1.orders.prod.c1.local", "type": "A", "value": "10.64.10.2" },
    { "name": "_http._tcp.api.orders.prod.c1.local", "type": "SRV",
      "priority": 0, "weight": 0, "port": 8080, "target": "api.orders.prod.c1.local" },
    { "name": "2.10.64.10.in-addr.arpa", "type": "PTR", "value": "api-1.orders.prod.c1.local" }
  ]
}
```

- Names are fully qualified without a trailing dot; forge-dns adds it.
- `zones` has one entry per cluster the file covers. On a node: one. On the
  server host: every cluster of every workspace.
- PTR records are generated for every A record whose address is inside a
  fabric subnet. Reverse zones are derived from the fabric subnets and
  listed in `zones` as `in-addr.arpa` names.
- Paths: node `<agent data_dir>/names.json` (`/var/lib/forge-agent/names.json`);
  server `<server data_dir>/names.json`. Written to `names.json.tmp` then
  renamed. Mode `0644`, owner the writing process.

### 2.4 `forge-dns`: new crate `src/apps/forge-dns`

- Binary `forge-dns`. Dependencies: `hickory-server` and `hickory-proto`
  with default features off plus what UDP/TCP serving needs, `serde`,
  `serde_json`, `tokio`, `clap`, `tracing`. **Do not add `notify`** (not in
  `Cargo.lock`): poll the file's mtime and size every 2 s and reload on change. **No OpenSSL, no DoT/DoH features.** `cargo deny` must pass;
  record the dependency in `DECISIONS.md`.
- Flags (also env `FORGE_DNS_*`):
  - `--file <path>` required.
  - `--listen <ip:port>` repeatable, required. On a node the agent passes the
    mesh address; on the server host, the address nodes reach the server on
    (`[dns] listen` in `forge.toml`, default empty = disabled).
  - `--upstream <ip:port>` repeatable. Default: the nameservers in
    `/etc/resolv.conf` that are not one of `--listen` and not `127.0.0.53`
    unless it is the only one.
  - `--ttl <secs>` overrides the file's `ttl`; default from the file.
- Behaviour:
  - Load the file at start; a missing or invalid file logs an error and
    serves REFUSED for the zones until a valid file appears.
  - On change, parse into a new map and swap it atomically. A parse failure
    keeps the previous map and logs.
  - A query whose name is in or under one of `zones`: answer from the map
    (A, SRV, PTR; AAAA for a known name answers NOERROR with no records);
    unknown name → NXDOMAIN with authority SOA. A records are rotated per
    query (round robin by a counter per name).
  - Any other name: forward to the first upstream, fall over to the next
    on timeout (1 s), copy the answer. No caching.
  - Max 512 bytes UDP without EDNS; respect EDNS size; TCP for the rest.
  - Rate limit nothing; bind only what is given.
- Unit `forge-dns.service`, written by `install.sh` for both roles:

```ini
[Unit]
Description=Orbyn Forge cluster DNS
After=network-online.target
Wants=network-online.target

[Service]
User=forge-dns
Group=forge-dns
DynamicUser=no
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadOnlyPaths=/var/lib/forge-agent/names.json
EnvironmentFile=-/etc/forge/forge-dns.env
ExecStart=/usr/local/bin/forge-dns --file ${FORGE_DNS_FILE} ${FORGE_DNS_ARGS}
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
```

  `/etc/forge/forge-dns.env` is written by the agent (node) or by
  `install.sh` (server). The agent rewrites it and runs
  `systemctl restart forge-dns` only when the listen address changes (first
  fabric apply, subnet change). `ReadOnlyPaths` uses the server path on the
  server host.
- The agent starts `forge-dns` only after the fabric is in sync and
  `names.json` exists. Node health (`facts`) reports `forge_dns: running|stopped|missing`.
- Packaging: `package.sh` builds `forge-dns-linux-<arch>` next to the agent
  and helper; the release feed lists it; `install.sh` installs it for both
  roles; the agent self-update path updates it like the helper.

### 2.5 Pointing clients at it

- Compose injection (helper, same place as `add_mesh_network`, only when
  `mesh_network` is set and `StackPart.dns` is set):

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub dns: Option<StackDns>,   // { servers: Vec<String>, search: Vec<String> }
```

  Writes `dns: [<node mesh ip>, <upstream_forge_dns...>]` and
  `dns_search: ["<stack>.<env>.<cluster>.<suffix>", "<env>.<cluster>.<suffix>", "<cluster>.<suffix>"]`
  on every attached service that does not set `dns` itself. Docker's
  embedded resolver in the container forwards to these in order and fails
  over quickly; nothing else is needed inside containers.
- Node itself (Linux): after `NetworkEnsure`, when `/run/systemd/resolve`
  exists run `resolvectl dns <iface> <mesh ip>` and
  `resolvectl domain <iface> ~<zone> ~<reverse zones>`. Interface-scoped, so
  the node's other resolvers are untouched. Without systemd-resolved: do
  nothing; the hosts-file block still serves the node. Document both.
- Windows nodes and nodes outside a fabric: hosts file only, unchanged.

### 2.6 Server host

`names::refresh` writes `<data_dir>/names.json` with every workspace's
records when `[dns] listen` is set in `forge.toml` (new section:
`listen = ["192.0.2.10:53"]`, `upstreams = []`). The server does not run
forge-dns itself; the unit does. `upstream_forge_dns` in `HostNames` is the
`[dns] listen` list.

### 2.7 Removal of a fabric

`FabricOp::Remove` also: stop and disable `forge-dns`, delete
`forge-dns.env`, run `resolvectl revert <iface>`, delete `names.json`.

### 2.8 Docs and records

- `concepts/local-names.md`: rewrite "How a node keeps them" into two paths,
  hosts file (every node) and forge-dns (fabric nodes); the names table gains
  stack and replica rows; the search list; the health rule.
- New `reference/forge-dns.md`: flags, env, file format, unit, ports.
- `reference/configuration.md`: `[dns]`.
- `guides/cluster-network.md`: remove "no internal DNS resolver yet".
- `reference/agent-protocol.md`: `HostNames` fields 4–6, `StackStates`.
- `DECISIONS.md`: forge-dns as a standalone binary; hickory dependency;
  name shape and coexistence rule; replica slot from Compose; sick replica
  rule.
- `ARCHITECTURE.md`: forge-dns on nodes and server host; SVG.

### 2.9 Tests

- `forge-catalog`: `Records` round trip; search list builder; name rules
  (clash of replica with service).
- `forge-server`: `build_fleet` rows for a stack with 2 healthy and 1
  unhealthy replica; coexistence; PTR generation; `names.json` written on
  refresh when `[dns] listen` is set.
- `forge-dns`: unit tests with an in-process client: A round robin, NXDOMAIN
  in zone, forwarding out of zone, file swap, invalid file keeps old map.
- Lab: from a container on lab-b, `getent hosts api.orders.prod` and a
  short name through search; stop a replica and see it leave the set within
  10 s; stop the agent on lab-c and resolve through tier 2.

## 3. Step 3: `forge.stack.yaml` and `forge stacks validate / plan / apply`

### 3.1 Document

Parsed with `forge_server::deploy::parse_yaml` (same bounds), then into a new
`forge_catalog::stack_doc::StackDoc` (serde, `deny_unknown_fields`):

| Field | Type | Required | Rule |
| --- | --- | --- | --- |
| `apiVersion` | string | yes | `forge/v1` |
| `kind` | string | yes | `Stack` |
| `metadata.name` | string | yes | `valid_stack_name` |
| `metadata.environment` | string | yes | environment slug; `--environment` overrides |
| `spec.compose.file` | string | one of `file`/`inline` | path relative to the document, no `..`, read by the CLI and sent as text; the server never reads files |
| `spec.compose.inline` | string | one of | the compose document |
| `spec.compose.envFile` | string | no | same path rules; sent as the `.env` text; sealed server-side as today |
| `spec.placement.nodes` | list of strings | no | node names as `resolve_node` accepts; the first is the stack's `default_server`; others only matter for `x-forge.node`-less services via `x-forge.cluster`? **No:** with more than one node and no `x-forge` keys, refuse: `placement.nodes lists several nodes; name the node per service with x-forge.node or use placement.cluster` |
| `spec.placement.cluster` | string | no | cluster name; sets top-level default placement to every node, equivalent to `x-forge.cluster` on every service without a placement. Exclusive with `nodes` |
| `spec.network.mesh` | bool | no, default true | maps to top-level `x-forge.mesh` |
| `spec.rollout.order` | `node-by-node` \| `all-at-once` | no, default `node-by-node` | |
| `spec.rollout.healthTimeout` | duration | no, default `5m` | 10s–1h; passed as `--wait-timeout` to compose |
| `spec.overrides.<env>` | object | no | deep-merged over `spec` when `--environment <env>` or `metadata.environment == env`; only `placement`, `network`, `rollout`, `compose.envFile` may appear |

Anything else is refused with the field path.

### 3.2 API

- `POST /api/v1/stacks/plan`, `stack.manage` on the environment. Body
  `{ document: <yaml text>, compose: <text>, env: <text|null>, environment?: <slug> }`.
  Returns `{ action: "create"|"update", stack_id?, environment_id, nodes: [...NodePlan preview],
  mesh: { enabled, nodes_in_sync: [...], nodes_not_in_sync: [...] }, findings: [...],
  revision_next: n }`. No side effects.
- `POST /api/v1/stacks/apply`, same body plus `{ order, health_timeout_s }`.
  Does: create or update the stack (existing paths), save the revision with
  note `apply <sha256 of document>[:12]`, start the rollout, return
  `{ job_id, stack_id, revision }`.
- Both audit as `stack.plan` (plan) and the existing create/update/deploy
  actions.

### 3.3 Rollout job

New job kind `stack.rollout` in `jobs/`, handler `StackRolloutJob`, steps
`["next"]` repeated: payload `{ stack_id, revision, nodes: [server ids in order], order, health_timeout_s, index }`.
Each `next` step dispatches the existing per-node `stack.action` deploy for
`nodes[index]` and waits for it (`order: node-by-node`), or dispatches all
and waits for all (`all-at-once`). A failed node fails the rollout; nodes
not yet started are not deployed. Every step re-authorizes `stack.manage`.
Idempotent: a step that finds a node already at `revision` with state rows
present skips it.

### 3.4 CLI

In the stacks command module: `validate -f`, `plan -f [--environment]`,
`apply -f [--environment] [--wait] [--non-interactive] [--yes]`. `apply`
prints the plan, asks for confirmation unless `--yes` or
`--non-interactive`, starts, and with `--wait` follows the job like
`forge deploy apply --wait` does (`deploycmd::follow`). On success prints:

```
stack orders revision 7 is live on 2 nodes
names:
  api.orders.prod.c1.local      10.64.10.2, 10.64.12.4
  worker.orders.prod.c1.local   10.64.10.3
```

from `GET /api/v1/names?stack=<id>` (new filter on the existing endpoint).
Exit codes follow `deploy`: `0` live, `1` plan refused, `7` a rollout is
already running, `2` a node failed.

### 3.5 Docs and tests

- `reference/stack-yaml.md` (every field, as the table above),
  `guides/deploy-stack-with-yaml.md`, `guides/deploy-from-ci.md` gains the
  stack variant, `reference/cli/stacks.md`, `rest-api.md`.
- Tests: document parsing and overrides; plan returns findings for a cluster
  without a fabric; rollout stops after a failed node; `it` test for authz
  (`stack.manage` only; `workspace.manage` not required).

## 4. Order of work and what each PR must contain

1. Step 1 (one PR, may be split in catalog+agent then server+UI). Docs in the
   same PR. Lab verification recorded in the PR description.
2. Step 2a records only, still through the hosts file (`names.rs`,
   `HostNames` fields, stack state push). Names work before forge-dns exists.
3. Step 2b `forge-dns`, packaging, injection of `dns`/`dns_search`,
   resolvectl, `[dns]` config, DECISIONS entry.
4. Step 3 document, API, rollout job, CLI, docs.

Each PR passes the gate in `CLAUDE.md`. No step changes a decision of an
earlier step; if one seems to need it, stop and ask.
