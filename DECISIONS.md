# Decision log

Running record of architecture and dependency decisions. Newest last. Each
entry: context → decision → consequences.

## D-001 Hexagonal core with ports in `forge-core`
**Context.** Jobs, audit, identity and plugins all need storage and
authorization; later phases add PostgreSQL, KMS, OpenBao.
**Decision.** Repository and authorizer traits live in `forge_core::ports`.
Services depend only on these; `forge-store` implements them; `forge-server`
is the single composition root.
**Consequences.** Swappable adapters and fakes in tests. Some `async_trait`
boxing overhead, negligible next to I/O.

## D-002 `sqlx::Any` with one portable migration set
**Context.** SQLite by default, PostgreSQL behind a feature, same migrations.
**Decision.** Use `AnyPool`, runtime queries (no compile-time `query!`, which
needs a live DB), and the shared SQL subset: `$n` placeholders, `TEXT`/`BIGINT`
only, flags as 0/1, epoch-ms timestamps, `ON CONFLICT`, `RETURNING`.
**Consequences.** No JSON/UUID native types; values are parsed in Rust. The
`postgres` feature only adds the driver.

## D-003 Roles as Cedar templates linked per binding
**Context.** Resource-scoped roles across Platform/Org/Tenant/Project/Env.
**Decision.** Roles are action groups; each binding links a role template
(`principal == ?principal, resource in ?resource`). Guardrails are `forbid`
policies. All policies are strictly validated at startup.
**Consequences.** Granting or revoking is a link/unlink, auditable, and needs no
policy text edits. Requests that don't fit the schema fail closed.

## D-004 Agent link: hyper HTTP/2 + `prost` messages, not tonic
**Context.** Spec asked to measure tonic vs prost + h2. `protoc` is not
available on all build hosts, and tonic adds tower layers and codegen.
**Decision.** Messages are `prost` derives in `forge-proto` (a reference
`.proto` is kept in sync for other languages). Transport is hyper HTTP/2 over
rustls with request/response heartbeats that carry pending commands.
**Consequences.** No build-time protoc, fewer dependencies, smaller agent.
Measured memory is in BENCHMARKS.md. Moving to tonic streaming later is a
transport swap; the message types stay.

## D-005 `ring` as the single crypto provider
**Context.** rustls's default provider (aws-lc-rs) needs cmake/NASM on Windows
and carries extra licenses; the plan says "no OpenSSL".
**Decision.** rustls, tokio-rustls and rcgen use `ring`. `ring` also provides
SHA-256, HMAC (TOTP), AES-256-GCM (envelope encryption), Ed25519 (command and
audit signatures) and the CSPRNG, so there's no separate RustCrypto stack.
**Consequences.** One audited crypto library. `ring` is Apache-2.0 AND ISC
(ISC is a permissive BSD-style license, allowed in `deny.toml`).

## D-006 Passkeys behind a trait; `webauthn-rs` deferred
**Context.** `webauthn-rs` depends on `openssl`, which conflicts with D-005 and
the "no OpenSSL" rule. The spec requires asking before such dependencies.
**Decision.** `forge-identity` defines a `PasskeyVerifier` port and exposes the
capability as `auth.passkeys: false`. TOTP MFA ships now.
**Consequences.** **Open question for the owner:** accept `webauthn-rs`
(OpenSSL) or adopt a pure-Rust WebAuthn verifier (to be evaluated).
**Superseded by D-055.**

## D-007 Plugins: one runner process per invocation, no WASI
**Context.** "Installed ≠ running"; inactive plugins must cost zero.
**Decision.** The host spawns `forge-plugin-runner` per invocation, talks
line-delimited JSON over stdio, and the runner links **only** the
`forge:plugin/host` interface. Budgets: Wasmtime fuel, epoch deadline, memory
limit (trap on grow failure), output cap, wall-clock kill from the host.
**Consequences.** Zero idle cost and crash isolation; small spawn latency per
call (pooling is a later optimisation). OS-level cgroup/Job Object limits are
designed but not in Phase 1.

## D-008 Replay protection survives agent restarts
**Context.** An in-memory replay cache is lost on restart.
**Decision.** Commands carry `issued_at`/`expires_at` (≤ 5 min TTL) and a nonce.
The agent rejects anything issued before its own start time minus allowed
clock skew, plus anything in its bounded nonce cache.
**Consequences.** A restart can't be used to replay a captured command; the
control plane simply re-issues still-needed commands.

## D-009 Revocation checked in the application layer
**Context.** rustls supports CRLs, but rebuilding TLS configs per revocation is
heavy.
**Decision.** Short-lived agent certificates (24 h) with renewal at ⅔ lifetime,
plus a revocation table checked on every agent request by certificate serial.
**Consequences.** Revocation takes effect on the next request (< 1 heartbeat).

## D-010 License policy
**Decision.** `deny.toml` allows MIT, Apache-2.0, BSD-2/3-Clause, plus the
permissive equivalents ISC, Unicode-3.0, Zlib, CC0-1.0, BSL-1.0 and
CDLA-Permissive-2.0 (root certificate data only). Copyleft is denied.
**Consequences.** These additions go beyond the literal "MIT, Apache-2.0 or BSD"
rule; flagged for owner sign-off.

## D-011 Product license placeholder
`Cargo.toml` says `Apache-2.0` so tooling works. The product license
(open-core vs AGPL vs source-available) is still an open decision in the plan.

## D-012 Advisory exceptions and dependency upgrades
**Context.** `cargo deny`/`cargo audit` flagged (a) Wasmtime 38 (several
sandbox advisories), (b) `zbus` 5 helper crates (merged into `zbus` 6, now
unmaintained), (c) `rsa` (RUSTSEC-2023-0071, no fix released) pulled in by
`openidconnect`.
**Decision.** Upgrade to Wasmtime 49 (newest release that builds on the
current stable toolchain; 50 needs rustc 1.97) and `zbus` 6. Ignore
RUSTSEC-2023-0071 with a recorded reason: the Marvin attack targets RSA
private-key operations, while Forge only verifies IdP signatures with public
keys and never holds an RSA private key.
**Consequences.** Re-evaluate the `rsa` exception when a patched release
ships; bump Wasmtime to 50+ when the toolchain moves to 1.97.

## D-013 Organization renamed to Workspace; one workspace per deployment
**Context.** The product unit is a deployed control plane reached at its own
URL. Showing a workspace switcher inside the app added a layer users don't need.
**Decision.** "Org" was renamed "Workspace" end to end (types, Cedar,
`/api/v1/workspaces`, migration 0002). The UI hides workspaces: `init` creates
one, and the console uses it implicitly. The backend keeps the entity as the
internal root for policy and audit scoping.
**Consequences.** Multi-workspace hosting remains possible later without a
data migration; the UI stays simple (clusters → nodes).

## D-014 Clusters and multi-parent authorization
**Decision.** A Cluster is a named group of nodes inside the workspace. A node
can belong to a cluster *and* an environment, so `ResourceRef` carries a small
parent graph and the Cedar entity slice is built from it. Role bindings may be
scoped to a cluster.

## D-015 Credential-based node onboarding
**Context.** Users want to hand Forge a fresh VM's login (sudo user, no root)
and have the node join automatically.
**Decision.** Probe first (host key shown, nothing sent), user confirms the
fingerprint, the credential is sealed in a vault table with a one-hour TTL, and
the `node.bootstrap` job uploads the same installer the manual one-liner uses,
runs it with sudo, waits for the agent, then purges the credential unless the
user opted to keep it. After that the node is managed only via the agent's mTLS
identity. Requires the new `node.onboard` action, which is MFA-guarded. SSH is
`russh` (Apache-2.0, ring backend) in `src/adapters/forge-ssh`, server-side only.
**Consequences.** Windows assisted onboarding (WinRM/OpenSSH) is not built yet;
Windows uses the manual installer.

## D-016 One workspace per deployment; Environment renamed to Namespace; node roles
**Context.** A Forge deployment (its URL) *is* the workspace; exposing workspace
ids in every URL and a workspace switcher only added complexity. Users think of
test/PP/UAT/staging as *namespaces*, and the machine running Forge should be a
managed node like any other.
**Decision.**
- The API is flat (`/api/v1/clusters`, `/nodes/…`, `/projects`, `/namespaces`,
  `/enrollment-tokens`). The single workspace (created by `forge-server init`)
  is resolved implicitly by the `Ws` extractor and stays only as the internal
  root of the Cedar hierarchy and for `workspace:<id>`-scoped role bindings
  ("Everything"). `GET /api/v1/workspace` returns it.
- Environment → Namespace end to end (types, Cedar entity and actions, API,
  UI). Migration `0004_namespaces.sql` renames the table, columns and
  role-binding scopes; historical audit text is unchanged.
- Nodes have a role: `control` (hosts forge-server) or `worker`. The first
  agent that enrolls over loopback (the Forge host's own agent) becomes the
  control node; remote agents cannot reach the listener via loopback, so they
  cannot claim it. Names avoid master/slave and match Kubernetes/Nomad usage.
**Consequences.** Multi-workspace hosting would need a new decision; the data
model still carries `workspace_id`, so it remains possible.

## D-017 Browser SSH terminal with an SSH user CA
**Context.** Users want to log in to any node from the console, like PuTTY or
Windows Terminal, as a core feature.
**Decision.** Forge holds an Ed25519 *SSH user CA* (`data_dir/keys/ssh_user_ca`).
The node installer makes `sshd` trust it. Per session Forge generates a fresh
key and signs a five-minute certificate whose only principal is the requested
login, then connects with russh using the node's pinned host key and relays a
PTY over a same-origin WebSocket whose first frame must carry the CSRF token.
Authorization is the existing `terminal.open` action (MFA guardrail).
Endpoints live in `node_ssh` (migration 0005); assisted onboarding fills it.
The UI uses xterm.js 6 and its addons (all MIT), lazy-loaded.
**Alternatives.** A long-lived per-node key in the vault (a stolen key works
until rotated); relaying through the agent (needs new agent protocol and a
PTY in the agent, larger agent).
**Consequences.** Nodes must be reachable from the control plane on their SSH
port. Per-binding login restrictions and session recording are extension
points; SFTP upload is not built yet.

## D-018 App Store: compiled-in recipes run by a root helper service
**Context.** Users install databases, runtimes, web servers and platform
plugins (Docker, Swarm, K3s, RustFS, PowerDNS) on chosen nodes. That needs root
on the node, while the agent must stay unprivileged.
**Decision.**
- New crate `src/protocol/forge-catalog`: package manifests with typed
  parameters and one POSIX shell recipe per package (shared prelude). It is
  compiled into the control plane *and* the node helper.
- The wire command `PackageAction {package, version, action, params}` carries
  no script. The agent (allowlist entry `package`) forwards it to
  `forge-agent-helper serve`, a root service on `/run/forge-agent-helper.sock`
  (`root:forge-agent 0660`, peer-uid checked). The helper validates against its
  own catalog and runs the recipe with a clean environment, one at a time,
  45-minute limit, output tail returned.
- Package actions run in the background in the agent so heartbeats continue;
  results are reported on a later heartbeat. The `package.action` job waits up
  to 50 minutes and records the outcome in `installations` (migration 0006).
- Generated secrets stay in `/etc/forge/pkgs/<id>.env` on the node; only
  redacted variables are reported. `package.manage` is MFA-guarded.
**Alternatives.** Sending scripts signed by the control plane (flexible, but a
compromised control plane could run anything as root); SSH with sudo
(credentials on the control plane).
**Consequences.** New or changed recipes ship with agent releases. Recipes
support Debian/Ubuntu today; RHEL-family and Windows packages are future work.
The installer upgrades existing nodes (helper service, `package` allowlist).

## D-019 Containers through the helper and the Docker Engine API
**Decision.** Container management is built in for nodes that have Docker (an
App Store package). The root helper speaks the Docker Engine API over
`/var/run/docker.sock` with hyper (its `http1` feature enabled; no new crate)
and exposes a closed set of operations; the agent stays out of the `docker`
group. `ContainerAction` is a new wire command (allowlist entry `container`).
Reads need `server.manage`; changes need the new `container.manage` action (not
MFA-guarded: routine operations, no exec/run). Quick operations run inline in
the agent; image pulls run in the background.
**Consequences.** Creating arbitrary containers is not exposed; containers come
from App Store packages and, next, from Applications.

## D-020 Applications and environment delivery
**Decision.** An application (per namespace) is a *service*, *job* or
*container* described by `forge_catalog::AppSpec`, shared by the control plane
and the node helper. Environment variables are an ordered list; secret values
are sealed with the vault envelope (AAD `app:<id>:env:<key>`), never returned,
and unsealed only into the signed `AppAction` command. On the node the helper
writes `/etc/forge/apps/<ns>/<app>.env` (0600) used as `EnvironmentFile=` and
as the container environment, plus optional package env files. Native apps run
as a per-app system user; without systemd, `forge-agent-helper supervise`
keeps services running. Authorization uses the existing `deploy` action on the
namespace (plus `server.manage` to place an app on a node). Values may not
contain line breaks, so one file format serves systemd and Docker.
**Consequences.** Releases/artefacts (Git, uploads) and blue-green switching
come later; active/standby placement is recorded now for Sites and DNS.

## D-021 Settings and personal API keys
**Decision.** Instance settings are a key/value table (`settings`); bootstrap
values stay in `forge.toml` and are shown read-only. Applications reference
settings as `${forge.settings.<key>}`, expanded at deploy time. Personal API
keys (`api_keys`, migration 0008) are `fk_<prefix>_<secret>`, stored as
SHA-256, optionally expiring. They authenticate as their owner with
`mfa = false`, so MFA-guarded actions are impossible with a key, and a key
cannot mint keys. OIDC providers are configured in `forge.toml` (secrets from
the environment) and displayed in the console.
**Consequences.** Editing OIDC providers from the console would need the
client secret in the vault; deferred.

## D-022 Sites (Nginx) and DNS (PowerDNS) through the node helper
**Decision.** A site is a typed `forge_catalog::SiteSpec` (domains, active and
standby upstreams, balancing, TLS via certbot HTTP-01, limits) rendered by the
node helper, plus an optional free-form *custom block* in its own include file.
Each apply writes both files, runs `nginx -t`, reloads, and restores the
previous files if the test fails. Code-loading directives are refused in custom
blocks. DNS uses the PowerDNS HTTP API on the DNS node itself, through the
helper, with the API key read from the node's package env file (it never leaves
the node). Sites can publish A/AAAA records round-robin or active/standby.
`proxy.manage` (new, MFA-guarded) covers applying sites and DNS changes.
**Consequences.** Health-check driven DNS failover, custom certificates,
DNS-01/wildcards and Traefik/YARP are on the roadmap.

## D-023 Live node telemetry in memory, "watched" nodes report faster
**Decision.** Agents attach a `NodeMetrics` sample (sysinfo: CPU, memory with
cgroup limits, swap, per-volume disks, network rates, load) to each heartbeat.
The control plane keeps a bounded in-memory ring per node (3,600 samples) and
the latest volume list. Reading a node's metrics marks it *watched* for 20 s;
watched nodes get a 2 s long-poll hold instead of 25 s and are woken
immediately, so charts are live without a separate stream. Connectivity checks
are a new agent command `probe` (wire tag 18, read-only, allowlisted by the
installer) that pings peers from the node, falling back to a TCP connect.
**Consequences.** No metrics history across restarts and no alerting; a real
time-series store is a roadmap item. Peers are cluster members plus the
control node.

## D-024 Terminal sessions at the shell level; pop-outs relay over BroadcastChannel
**Decision.** Terminal sessions belong to a provider in the application shell
and render in a bottom dock, so navigation never closes them. A pop-out
window mirrors a session through a per-session `BroadcastChannel`: the main
window keeps the WebSocket, forwards output, accepts input and resizes, and
sends a serialized scrollback (`@xterm/addon-serialize`, MIT) when the window
opens. No second SSH connection or server-side session handover is needed.
**Consequences.** Closing the main window ends mirrored sessions; the pop-out
can reconnect on its own.

## D-025 Step-up MFA, confirmed revocation, display names
**Decision.** Denials caused only by the MFA guardrail are reported as
`step_up_required`; `POST /auth/step-up` verifies a TOTP code and replaces the
session with an MFA-verified one (throttled, audited), and the console retries
the request. Revoking a node requires the exact machine hostname and, for users
with an authenticator, a fresh code, both checked by the server. Nodes get an
optional `display_name` (migration 0010); the hostname stays the identity used
for probes and SSH.

## D-026 `dev_skip_mfa` for local development
**Decision.** An opt-in config flag treats interactive sessions as
MFA-verified so local development and the WSL lab need no authenticator. It is
rejected at start-up unless `public_url` is a loopback address, logs a
warning, is advertised as the `dev.skip_mfa` capability (the console shows a
banner) and never applies to API keys. The lab enables it.
**Consequences.** Production keeps step-up MFA unchanged; a deployment behind
a reverse proxy cannot enable the flag by accident because its public URL is
not loopback.

## D-027 Cluster profiles: orchestrator, overlay network, ingress layout
*Superseded by D-033: Swarm and K3s were removed.*
**Decision.** Each cluster gets one profile (migration 0011,
`forge_catalog::ingress::ClusterProfile`): an orchestrator (`swarm` by
default, `k3s` or `none`), an overlay network (name, subnet, encryption) and an
ingress layout: `standalone` (one edge), `active_standby` (one primary, VRRP
virtual IP via keepalived, or DNS) or `active_active` (every edge serves; one
VIP per edge or DNS round-robin). Forge does **no IPAM**: the subnet is handed
to Swarm (or K3s), which assigns container IPs, service VIPs and service DNS.
Native services are addressed as node address : port. The global Nginx config
and keepalived are rendered from typed values in Rust (unit-tested) by the
node helper, checked with `nginx -t` and rolled back on failure, through a new
`ingress` agent command. No new Cedar actions: saving a profile needs
`workspace.manage` on the cluster, applying needs `proxy.manage` (MFA) on each
edge and `container.manage` on the Swarm manager (network creation).
**Consequences.** Endpoints (sites) can belong to a cluster and take their
edges from the profile; their upstreams are typed targets (node service, Swarm
service, address) resolved to `host:port` when saved. The VRRP PASS value is
generated per cluster and kept server-side; VRRP sends it in clear, so it only
keeps other routers out of the group.

## D-028 Swarm workers join later, without exposing the join token
*Superseded by D-033: Swarm and K3s were removed.*
**Decision.** The setup wizard installs Docker on every member and initialises
Swarm on the chosen manager, but does not yet join the other members. Every
agent command result is persisted with its job, so fetching the worker join
token through a command would store a secret in the database.
**Next.** A dedicated join job that seals the token in the vault on the
manager's side (or relays it agent-to-agent) before any member joins.

## D-029 Namespaces belong to a cluster; the UI is cluster-scoped

**Decision.** A namespace gets an optional `cluster_id` (migration 0012). A
namespace is a family of services (applications) that runs in one cluster.
The sidebar has a cluster section, in the style of Rancher: a cluster switcher
with Overview, Nodes, Services (namespaces with their services) and Ingress,
all filtered to that cluster. "All services" and "All nodes" remain
fleet-wide views.
**Why.** Services, their nodes and their ingress always live inside one
cluster, so browsing them per cluster matches how they are run.
**Compatibility.** Existing namespaces stay unassigned (`cluster_id` NULL)
until they are added to a cluster with "Add existing namespace".

## D-030 Forge-managed cluster network (fabric)

**Decision.** Forge owns cluster addressing and internal DNS itself, without
Swarm or K3s. Each cluster gets one private subnet (/16 "class B" by default,
/24 "class C" for small clusters) from a pool (setting `fabric.pool`, default
`10.64.0.0/10`). The subnet is split into fixed regions: reserved/ingress
VIPs, node mesh addresses, service IPs and one instance block per node
(`forge_catalog::fabric::Layout`). Allocation is stable: existing leases are
kept, new owners take the lowest free slot, and manual pins are validated and
never moved. Subnets may not overlap other clusters or the networks the nodes
sit on.
**Transport.** By default an encrypted WireGuard mesh, which works across
clouds, data centres and home labs behind NAT (NATed nodes dial out, and a
public node relays between two NATed sites). "Routed" mode (plain routes, no
encryption) is an opt-out for private, trusted networks only.
**Approved by the owner (2026-11-09):** installing WireGuard on member nodes,
including the WireGuard for Windows driver, and the `hickory-dns` crate
(MIT/Apache-2.0) in the agent for the internal resolver.
**Resilience.** A reconciler job converges every node to the desired state
(install, interface, peers, routes, firewall, DNS) with idempotent steps and
exponential back-off (30 s up to 15 min), and re-runs when a node reconnects.
**Reconciler identity.** The reconciler submits `agent.command` jobs as the
principal who last pressed "Apply network", with the authorization context
(MFA) of that request. The job runner re-authorizes that principal before
every step, so revoking their rights stops it. The stored context is not a
session: it only lets the runner re-check the same request, as with any
queued job.
**Status.** Steps 1–2 shipped: address planning and pins, and the WireGuard
mesh (keys created on the node, direct / NAT / relay peers, install on Linux
and Windows, reconciler with back-off). Routes/firewall between namespaces,
service IPs, routed mode and the resolver follow.

## D-031 Credential vault for people (passwords revealed to platform admins)

**Decision.** Besides the system secrets Forge uses itself (never shown), the
vault keeps credentials for people: name, username, password, URL, notes,
tags and links to nodes, services and clusters. This is a deliberate,
owner-approved exception (2026-11-09) to "no secrets in the browser":
**platform administrators** may reveal a password.
**Safeguards.**
- The whole vault (list, create, edit, delete) is limited to users with a
  `platform_admin` binding. Refusals are audited.
- Revealing also needs `secret.read` (MFA guardrail, so a fresh step-up), and
  a browser session: API keys cannot reveal.
- Each reveal returns one password, `Cache-Control: no-store`, and is audited
  with the item id and name. The value is never logged or audited.
- Only the password is sealed (envelope encryption, AAD `vault:<id>`); notes
  are metadata and the UI says so.
- The UI shows a revealed password for 30 seconds, then drops it from memory.

## D-032 Bundled SQLite advisory (RUSTSEC-2026-0459) accepted until sqlx ships a fix

**Context.** SQLite before 3.54 can return stale process memory when a
database is opened together with a crafted sibling `-wal` file. sqlx 0.8.6
and 0.9.0 both pin `libsqlite3-sys` 0.30 (SQLite 3.4x); the fix needs 0.39.
**Decision.** Ignore the advisory in `deny.toml` and `.cargo/audit.toml`, and
make the server restrict its data directory (database, WAL, keys) to the
service user (`0700`) at startup. Forge opens only its own database there, so
whoever could plant a crafted WAL file already controls Forge.
**Revisit** on every sqlx release; drop the ignore once it bundles SQLite ≥ 3.54.

## D-033 No container orchestrator: Nginx profiles on the Forge cluster network
**Decision.** Docker Swarm and K3s are removed (packages, wizard step,
overlay-network agent operations, the Swarm network tab). The Forge cluster
network (D-030) already gives nodes and services addresses, internal names and
encrypted links, so an orchestrator added a second, overlapping network model.
Ingress profiles are Nginx only: `ClusterProfile` is `{ingress}`; profiles
saved with `orchestrator`/`network` still load and those keys are ignored.
Endpoint targets of kind `app` expand to every node a Forge service is placed
on, at the node's fabric address (else its own address), re-resolved on each
apply. Upstreams use addresses, not internal names, so edges do not depend on
resolver timing. Supersedes D-027's orchestrator/overlay parts and D-028.
**Revisit** when a concrete workload needs scheduling across nodes.

## D-034 Endpoint certificates and DNS failover
**Decision.**
- *Uploaded certificates* live in `tls_certificates` (migration 0017), not in
  the people-oriented vault (D-031): ingress operators (`workspace.manage`)
  use them, and the key never goes back to any browser. Upload checks the key
  against the leaf (`rustls` key provider), expiry and names (`x509-parser`,
  already a dependency). The key is sealed (AAD `tls-certificate:{id}`) and
  unsealed only in the `agent.command` dispatch step into the signed command,
  so job payloads and results hold the certificate id only. Edges write it
  `0600` under `/etc/nginx/forge/tls`.
- *Let's Encrypt on several edges:* each edge issues its own certificate.
  A challenge an edge does not hold is proxied once to its peers over the
  cluster network (a hop header prevents loops; a 404 moves to the next peer).
  No private key ever travels between edges. Certbot's timer renews, and a
  deploy hook reloads Nginx.
- *DNS:* an optional profile-level PowerDNS node writes A/AAAA records for
  every endpoint domain, using each edge's public IP and the longest matching
  zone. A background loop checks edges every 15 s (agent online + an HTTP answer on
  the public IP). It changes health after 3 agreeing results and rewrites the
  records when the wanted set changes: active-active lists the healthy edges;
  active-standby lists the primary, else the first healthy backup. If nothing
  is healthy, the records stay pointing at the edges (never empty). DNS
  commands run as the principal who last applied the profile, re-authorized
  by the job runner; each failover is audited (`ingress.dns.failover`).
  VRRP stays available for edges on one LAN.
- *Renewal guarantee:* every 6 h Forge reads each edge's Let's Encrypt expiry
  (`certbot certificates`). At 5 days left or fewer, or if the certificate is
  missing, it force-renews on that edge, runs `nginx -t` and reloads. Failures
  are retried hourly. This runs as the principal who last applied the
  endpoint, and each renewal is audited. Uploaded certificates cannot be
  renewed by Forge; they warn at 14 days.

## D-035 Packages come from repositories; Forge tracks builds and offers updates
**Context.** Vendors ship hotfixes, minor and major releases continually.
Users need to see, per node, which installed package has an update and apply
it quickly, without losing the ability to update by hand on the node.
**Decision.**
- *Package managers first:* App Store packages install from the OS package
  manager (apt today, dnf/yum next, winget on Windows) and run as the
  distribution's service. Binary/tarball installs remain only where no vendor
  repository exists and are shown as "manual updates" (no `OS_PKG`).
- *Node facts are the truth for "what can I get":* recipes declare `OS_PKG`;
  the read-only `versions` action reports the installed build, the candidate
  the node's repositories offer, and the version the running service reports
  (`pkg_running`). Builds are ordered with the dpkg algorithm
  (`forge_catalog::versions`) and classified as revision, patch, minor or major.
- *Pinned updates:* `POST /installations/{id}/update {build}` accepts only the
  node's current candidate and passes it as the reserved `build` parameter
  (validated charset). The `upgrade` action installs exactly that build
  (`--only-upgrade pkg=build`, keeping existing config files), restarts or reloads,
  fails if the package ends at another build, and the job fails if the service
  still reports the old version. A retry installs the same build.
- *Vendor release lines* (LTS, latest, EOL) come from endoflife.date (data
  under MIT, sourced from vendor lifecycle pages), refreshed daily into
  `package_releases`. Forge keeps the last good data if the fetch fails, and
  update detection works without it. When the vendor's latest is newer than anything the
  node's repositories offer, the UI says so ("released upstream, not on this
  node yet").
- *Policy:* `notify` by default; `auto_patch` (opt-in per installation)
  applies revision and patch updates on the six-hourly pass. Minor and major
  updates always need a click and a downtime confirmation. Background checks
  and automatic patches run as the principal of the installation's last job,
  re-authorized by the job runner, and are audited (`package.update`).
- `reqwest` moves from a dev-dependency to a normal dependency of
  `forge-server`. It was already in the binary through `forge-identity`.
**Consequences.** Migration 0018 adds `installations.versions` and
`update_policy` and the `package_releases` table. Converting the remaining
recipes, dnf and winget sources, and new catalog packages follow in small steps.

## D-036 Packages on RHEL and Windows; cluster names through hosts files
**Decision.**
- *One recipe per package, both Linux families:* recipes branch on
  `os_family` (deb or rpm) and use shared helpers (`os_install`, `rpm_repo`,
  `epel_enable`, `fw_open`, `selinux_port`, `port_free`). Each recipe
  declares the packages that are released together (`UPGRADE_WITH`); an
  update moves them to the same build, but only those on the current build.
  Every recipe runs in `scripts/lab/recipe-test.py` on systemd containers of
  Debian 12, Ubuntu 24.04 and Rocky 9 (install, status, versions, upgrade,
  restart, uninstall).
- *Windows uses winget, run by the agent service.* The service already runs
  as LocalSystem, so there is no separate helper. The catalog lists winget ids
  per package (several, tried in order, with the release line expanded). One
  PowerShell runner (`_winget.ps1`) implements the same actions and output as
  the shell recipes, so versions, pinned updates and the Updates page work
  unchanged. Generated secrets go to `%ProgramData%\Forge\pkgs` with an ACL
  for SYSTEM and Administrators only. Only packages Forge can configure
  silently are offered on Windows (PostgreSQL, Node.js, Temurin, .NET).
- *Cluster names via hosts files, not a DNS server:* the fabric config each
  node receives lists node and service names with their mesh addresses. The
  agent keeps them in a marked hosts-file block, rewritten in place (it is
  bind-mounted in containers). This needs no resolver changes, works the same
  on Linux and Windows, and follows placements within one reconcile pass.
- *DNS ownership:* when the cluster profile publishes DNS, endpoint-level DNS
  is not published (the profile's failover would be overwritten).
- *Container logs follow* with a `since` cursor (new proto field
  `ContainerAction.since = 5`, validated as a decimal Unix time) instead of
  re-reading the tail.

## D-037 Self-contained releases and one-click self-update
**Decision.**
- *Self-contained:* the web console is compiled into `forge-server`
  (`build.rs` embeds `src/ui/dist`; no new dependency). A release package
  carries the server, the plugin runner, all agent binaries (`dist/`, found
  next to the server), an installer and an example configuration. The Linux
  installer sets up a systemd service; the Windows installer a SYSTEM
  scheduled task at boot (no Windows-service code in the server).
- *Signed releases:* CI publishes raw binaries, packages, `SHA256SUMS` and an
  Ed25519 signature over it. The public key is compiled in
  (`RELEASE_PUBLIC_KEY`); the private key is the CI secret
  `FORGE_RELEASE_KEY`. Signing and verification use `ring`, the one crypto
  provider.
- *Self-update* (`system.update` on the platform: platform administrators,
  MFA step-up, audited): check the release feed (GitHub latest release, every
  6 h), then download, verify each file against the signed sums, dry-run the
  new binary against the configuration, back up SQLite (`VACUUM INTO`), swap
  binaries by rename (old kept as `*.prev`) and restart in place. Unix
  `exec`s the new binary (same PID, so systemd, Docker or a shell keep
  supervising it); Windows starts a new process and exits. A start counter in
  `data/updates/applied.json` restores `*.prev` after three failed starts.
- Agents are not updated by this. The new agent binaries are served to nodes
  that install or reinstall.

## D-038 Migrations squashed into one file before the first release
**Decision.** Migrations 0001-0018 are squashed into
`0001_initial.sql`, in their original order, before Forge has been released.
The "never edit a released migration" rule still applies from here on. Later
schema changes are new numbered files.
**Consequences.** Databases created before the squash (development and lab
installs) record the old migration history and do not start with this build.
Recreate them (`scripts/lab/down.sh --reset`, then `up.sh` and `seed.py` in
the lab).


## D-039 Local release feed for lab testing
**Decision.** `scripts/setup_testing/` publishes local builds as GitHub-style
releases from the developer's machine (`publish.ps1` builds in WSL and signs;
`feed_server.py` serves the GitHub release API and download URLs, accepting
any or no access token). Lab servers install from the feed and self-update
through the real updater by setting `[update] feed_url` and `public_key` to a
local dev key, which lives git-ignored in `scripts/setup_testing/.keys/`.
Each local build needs a version newer than the last, so `forge-server` and
`forge-agent` take an optional compile-time `FORGE_BUILD_VERSION`
(`<Cargo version>-dev.<yyyyMMdd.HHmmss>`, which Forge's Debian-style ordering
puts after the base version and before the next release). Without it the
crate version is used, as before.
**Consequences.** No new dependency; the release workflow is unchanged. The
dev key must never be used for real releases. The feed is plain HTTP on the
lab network; integrity comes from the signature, as with GitHub.

## D-040 Usernames, and a generated first administrator
**Decision.** Accounts have an optional `username` and an optional `email`
(at least one; both unique, lowercase), and sign in with either: the login
field is a username, or an email when it contains `@`. The installers no
longer ask for an email or a password: they create the platform
administrator `admin` (`--admin-username` / `-AdminUsername`) with a
generated 24-character password, print it once in the install summary and
save it to `<data_dir>/initial-admin-password` (root, or SYSTEM and
Administrators, only). `forge-server init --generated-password` records whose
password it is; Forge deletes the file when that user changes the password.
`FORGE_ADMIN_PASSWORD` still lets an operator choose the password (nothing is
saved then). Users change their own username, email, display name
(`PATCH /me`) and password (`POST /me/password`, current password required)
in Settings; both are audited, including refusals. `init` now validates the
names and the password before it creates the database, so a rejected
password no longer leaves an installation without an administrator.
**Consequences.** Migration 0001 was edited (email nullable, `username`
added) because no release has shipped; existing development and lab
databases must be recreated, as after D-038. The API accepts `login` (and
still `email`) at `/auth/login`, and a username in place of an email when
granting roles. A first sign-in does not force a password change.

## D-041 Object storage: managed RustFS and connected S3 services
**Decision.** Forge tracks *object stores* (GitHub issue #1). A store is
either `managed`, a RustFS instance Forge installs and runs on a node, or
`external`, an existing S3 service (AWS S3, another S3-compatible cloud, or
a node Forge does not manage) connected with an access key. Each managed
store has *members* (node + exclusive data path, the volume attachment) and
is an instance named by a slug, so several stores share a node with
distinct service or container names, ports, settings and data paths. The old
App Store `rustfs` package stays for now; adopting it is later work.
- **Runtimes.** Linux: the release zip from GitHub (checked against the
  release's `SHA256SUMS`) as a systemd service under a dedicated user, or the
  `rustfs/rustfs` image on Docker. Windows: `rustfs.exe` as a SYSTEM
  scheduled task (RustFS ships no Windows service wrapper), native only,
  x86_64; implemented but not yet validated in the lab. The node runs a new
  `StorageAction` command (closed `StoreOp` set, validated by the agent and
  the root helper); the provider scripts are compiled into the helper.
- **Missing mounts.** With *require a dedicated mount*, install and start
  refuse a data path that is not its own mount point, and systemd checks it
  before every start. Uninstall never deletes data.
- **Credentials.** The control plane generates the bootstrap (root) key and
  seals it as a vault item; the job payload holds ids only and the key is
  unsealed into the signed command for `install`. Connected stores keep the
  entered key the same way. The item outlives an uninstall (recovery).
- **S3 calls** (buckets, objects) run from the control plane with a new
  adapter crate, `forge-s3`: SigV4 signed with `ring` (no second crypto
  library), `quick-xml` (MIT) for responses, bodies streamed with
  `UNSIGNED-PAYLOAD`. Transfers stream through `forge-server`: at most 8 at a
  time, 5 GiB per upload (single PUT). Presigned and multipart transfers are
  later work.
- **Authorization.** New actions `storage.manage` (MFA guardrail; install,
  lifecycle, connect, bucket deletion) and `storage.data` (browse, transfer,
  create buckets; developers too), checked on the member node for managed
  stores and on the workspace for connected ones. Everything that changes
  state is audited, and downloads too.
**Consequences.** New migration `0002_object_storage.sql`; reqwest gains its
`stream` feature. Agents enrolled before this need `storage` in
`allowed_commands` (the node installer adds it on upgrade). Managed
endpoints are plain HTTP on the private network until TLS for stores lands.
App credentials and Object Explorer extras
(versions, multipart) follow in later slices.

## D-042 Object storage high availability with distributed RustFS; no Garage
**Decision.** Managed stores have a topology: `standalone` (one node) or
`distributed`, a RustFS erasure-coded cluster across Linux nodes that keeps
the members in sync. A distributed store needs at least 2 nodes and 4 drives
(data paths) in total, the same number of drives on every node, at most 32
nodes. Forge gives every member the identical `RUSTFS_VOLUMES` list (every
drive of every member as `http://<endpoint_host>:<port><path>`, in member
order), the same port and the same credential; each member installs in its
own job, in parallel, and waits up to 15 minutes for quorum. Docker members
use host networking so the listed addresses are their own. The store's
status is aggregated from its members (`degraded` while only some run), and
Forge's S3 calls use the first reachable member.
Garage is dropped from the plan, a scope change from issue #1 that the user
approved ("if Garage is overkill and RustFS covers it, remove it"): RustFS's
distributed mode covers high
availability, and a second managed engine would double the install, upgrade
and test matrix for no capability Forge needs. An existing Garage service can
still be connected as an S3 store.
**Consequences.** Members carry `data_paths` (several drives) and an
`endpoint_host`. Members must resolve and reach each other on the S3 port;
Forge does not yet create a load balancer or DNS name for applications.
Converting standalone to distributed, adding pools and replacing nodes are
later work.

## D-043 File explorer: one file service, roots, confinement on the node
**Decision.** The file explorer (issue #2) is built around one control-plane
service, `forge-files::FileService`, which both the HTTP API and the SFTP
gateway use: it resolves roots (the browser sends only a root id), checks
every Cedar action an operation needs, refuses changes to read-only roots,
applies the protected-path policy to every path, binds uploads to their
creator and writes the audit log. File operations travel as a signed agent
command, `FileAction` (tag 22, allowlist name `files`), carrying a
`forge_core::files::FileOp` bound to a `RootSpec` (`node`, a directory, or a
directory inside a container). The node validates and applies the policy
again. On Linux the root helper runs every operation, so the checks run inside
the helper; on Windows the agent service (LocalSystem) runs it in process.
Agents run file commands in the background, so a long copy or search never
holds up heartbeats.
Linux resolves paths component by component with `openat(O_NOFOLLOW)` from an
open directory handle (`rustix`) and changes entries with `*at` calls on the
parent handle (`mkdirat`, `renameat2(RENAME_NOREPLACE)`, `unlinkat`), never
following a link. This closes the check/use race without `openat2` and makes
the written path the real one. Files with several hard links are compared by
inode against the protected files at the moment they are opened. Windows
refuses reparse points on the way and re-checks the open handle. Container
roots go through `/proc/<pid>/root` of the container's init process with the
same resolution, bound to the full container id; this needs no shell in the
image and no Docker overlay internals, but the container must be running.
Roots: the built-in `node` root covers the whole node minus fixed protected
paths, and needs `file.node`, which the user decided only `platform_admin`
and `workspace_admin` get. Administrators approve application and container
directories (`file.admin`, MFA, migration `0003`), read-only by default;
operators reach only those. `file.read` needs no MFA; `file.write` and
`file.delete` (admins, operators) and `file.admin` need MFA (guardrail).
Saves are atomic (private temp file in the same folder, SHA-256 checked,
owner and mode copied, renamed over the original) and refused when the
version token no longer matches. Uploads are resumable 256 KiB chunks into a
private `.forge-upload-<id>.part` file, verified by size and SHA-256 at
commit. Deletes go to a trash in the agent's protected data directory (30
days, 10 GiB per node); the last 10 versions of files up to 2 MiB saved from
the console are kept as revisions. Search runs on the node with bounded
memory (64 KiB reads, at most 8 MiB per file), at most two per node at low
priority on Linux, and one per user per node.
**Consequences.** Admins can change any non-protected file on nodes in their
scope; operators see nothing until a directory is approved. ACLs and
extended attributes are not preserved on save. On Windows a folder swapped
for a junction between the check and the open is not detected (the final
file is), and searches run at normal priority because changing thread
priority needs FFI the workspace does not allow outside the SCM module.
Changes in a container are lost when it is recreated; containers have no
trash or revisions. Validate-and-apply workflows for Nginx and systemd are
later work.

## D-044 File transfer over agent heartbeats until a streaming channel exists
**Decision.** File bytes travel inside command results: reads and upload
chunks are at most 256 KiB, base64-encoded, so several results fit under the
1 MiB heartbeat body limit. The file service calls the agent hub directly (no
job per listing), waiting 30 s for reads and up to 10 minutes for copies,
moves and commits. Downloads stream chunk by chunk, pinned to the first
chunk's version token (device, inode, size and timestamps on Linux; size and
timestamps on Windows), and are capped at 512 MiB. Downloads are
`application/octet-stream` attachments with `nosniff` and
`Content-Security-Policy: sandbox`; image previews use `data:` URLs.
Every call is audited with path and size (one event per download); refusals
with the reason; contents and search patterns never.
**Consequences.** Transfers are slow (one long-poll round trip per 256 KiB)
and share the agent link with management commands. A dedicated,
agent-authenticated streaming channel would speed up large transfers and the
SFTP gateway; it is not built yet.

## D-050 CodeMirror 6 for the file editor instead of Monaco
**Decision.** The console's file editor uses CodeMirror 6 (`@codemirror/*`,
MIT), not the Monaco editor named in issue #2; the user chose it. It covers
syntax highlighting (JSON, YAML, TOML, INI/.env, shell, PowerShell, Python,
JavaScript/TypeScript, HTML, XML, CSS, SQL, Nginx, Dockerfile, systemd),
find and replace, multiple cursors, folding and a side-by-side merge view for
save conflicts, in one lazy-loaded chunk of about 230 KB gzipped bundled with
the console (air-gapped installs need nothing else). Monaco would add
several MB and web workers for the same use. Vim keys and Markdown are left
out to keep the dependency set small.
**Consequences.** Language support is CodeMirror's; formatters are not
included. The merge view compares the file as it is now with your edit;
the original is available from the revisions.

## D-046 Onboarding console, cluster placement at enrollment, cluster handover
**Decision.** Node installs show a shell-style console: every command as a
`$ …` line and its output, live. Assisted (SSH) onboarding runs the
installer detached on the node (`setsid nohup`, in a private 0700 directory)
and follows its log every 1.5 s over the same SSH session; the lines go to a
bounded in-memory console (`/api/v1/onboarding/console`) and a copy stays in
the job's state. Manual installs report their steps to
`POST /api/v1/enroll/progress`, authenticated by their enrollment token (sent
from a 0600 header file, never on a command line; accepted until an hour
after the token expires). App Store recipes print their commands the same
way (the prelude's `step`). Secrets are masked before a line is stored.
A node now lands in its cluster when it enrolls: assisted onboarding assigns
it as soon as the token is used (not only once online), and manual tokens
record the chosen cluster (`enrollment_targets`, migration 0006). Re-onboarding
a machine that is already enrolled updates it in place: the installer reports
the identity it runs as and the job continues with that server.
Moving a node to another cluster is a handover: refused while the node is
the old cluster's remote-access gateway or ingress edge; otherwise the node
leaves the old cluster network (WireGuard interface removed) and joins the
new one.
**Consequences.** Live console lines are lost on a control-plane restart
(the job keeps its copy). The installer needs `curl` to report progress;
without it the install still works, silently.

## D-047 Cancel stops the work, not only the job record
**Decision.** Cancelling a job wakes its running step at once (the engine
keeps a per-job signal) instead of at the next lease renewal, and sends the
node a signed `CancelAction` (agent protocol tag 25) for the job's
operation. The agent aborts that background operation; the root helper sees
the connection close and kills the recipe's process group (recipes run in
their own group), so `apt-get`, downloads and installers stop too. Object
store and remote-access commands now run in the background on the agent,
like package recipes, so they never hold up heartbeats.
**Consequences.** A cancelled step may leave a half-done change on the node;
steps are idempotent, so running the action again repairs it. Agents
enrolled before this need `cancel` in `allowed_commands` (the node
installer adds it).

## D-051 Forge-managed SFTP gateway for temporary file access
**Decision.** Temporary file access (issue #2, step 5) is served by Forge's
own SFTP server, `forge-file-gateway`, running inside `forge-server` when
`[sftp] enabled = true` (port 2022 by default). It uses `russh` (already in
the tree, `ring` backend, no OpenSSL) and `russh-sftp` 3 (Apache-2.0, pure
Rust; approved by the user). Only password authentication and the `sftp`
subsystem are offered: no shell, exec, PTY or forwarding. The user name is the
grant id; the password is a 32-byte random token shown once and stored as a
SHA-256 hash (high-entropy token, compared in constant time). Grants live in
migration `0009` and record roots, write/delete rights, expiry (at most
`max_hours`, default 7 days), allowed client addresses, session, bandwidth
and transfer limits, and usage. Issuing needs MFA and every right the grant
carries. Each SFTP request runs through `FileService` as the grant: the grant
is re-read on every operation and authorized as its creator (with the MFA
context of issuance), so effective access is the intersection of the grant,
the creator's current roles and the root policy. A sweeper closes sessions
every 5 s when their grant expired, was revoked or lost its creator's
`file.read`. Failed logins are throttled per address. Uploads are buffered
into 256 KiB chunks with a running SHA-256 and committed on close; writes must
be sequential; replacing needs the delete right; `rm` moves to the trash. The
Ed25519 host key is kept in the data directory and its fingerprint is shown
with every grant; rotation is delete-and-restart.
**Consequences.** V1 runs beside the control plane and relays bytes over the
agent link (D-044), so throughput is limited; node-local gateways are later
work. Segmented or parallel uploads of one file are refused. Times and modes
set over SFTP are ignored. FTP and FTPS are not supported.

## D-052 Agent self-update as a signed command with pinned checksums
**Decision.** Agent self-update (issue #4) is an `agent.update` job that
sends the node a signed `AgentUpdate` command (command name `update`). The
command carries the target version, the download base URL, the architecture
and the SHA-256 of this release's `forge-agent` and `forge-agent-helper` for
that architecture, so the checksums are pinned per release by the control
plane's signature, not fetched beside the binaries. The node validates the
spec (hex checksums, `x86_64`/`aarch64`, a safe version string, an http(s)
base URL without a query), then downloads with `curl` using a fixed argv:
`--proto =http,https`, `--max-redirs 0`, a 10-minute time limit and a
200 MB size cap. A mismatch installs nothing. The job is authorized as
`server.command` (MFA) on every step.
**Consequences.** No new dependency on the agent. Nodes need `curl` and a
direct route to the control plane; redirects and mirrors are refused.

## D-053 The helper swaps the binaries; the agent confirms after a heartbeat
**Decision.** The root helper, not the agent, performs the update: it
stages and verifies the binaries, dry-runs `forge-agent --version`, copies
them next to `/usr/local/bin/*` and renames them in, keeping `*.prev`, and
writes `update-pending.json` in the agent state directory. It restarts both
services through `systemd-run --on-active=3` so the reply reaches the
control plane before the processes stop. The new agent confirms after its
first good heartbeat; the helper (once it runs the new version too) then
removes the marker and the backups. The job's `await` step waits for the
node to report the target version.
**Consequences.** Self-update needs systemd; Windows agents are updated by
reinstalling. Jobs running on the node during the restart can fail and must
be re-run (steps are idempotent).

## D-054 Watchdog rollback on a crash loop or a 5-minute deadline
**Decision.** While `update-pending.json` exists, the helper runs a
watchdog. It restores the `*.prev` binaries and restarts both services when
the new agent has not confirmed within 5 minutes of the swap, or when the
helper has started more than `MAX_STARTS` (3) extra times with the marker
present. The marker is kept as `update-rolled-back.json` for diagnosis.
**Consequences.** A bad release cannot strand a node: it returns to the
previous version without operator action, and the control-plane job fails
with a message that says so.

## D-057 File manager essentials: selection, permissions, archives, previews
**Decision.** Issue #2 step 6 adds multi-selection with bulk copy, move,
trash, delete, compress and download-as-archive (one API call per batch, a
result per item, up to 500 items); Linux `chmod`/`chown` as `file.admin`
(MFA) with recursive apply and a dry-run preview, done through the confined
backend (`fchmod` on a no-follow handle, `fchownat` without following links);
read-only Windows ACLs from `icacls` (no shell); and archives with `zip`
9 (MIT, deflate only) and `tar` 0.4 (MIT/Apache-2.0) over the `flate2` already
in the tree; no OpenSSL. Extraction validates every entry name with the
normal path rules, refuses absolute paths, drive letters and `..`, skips
links and device files, caps entries (20,000), size (4 GiB) and ratio (200:1),
writes into a hidden temporary folder through the no-follow backend and
renames it into place only at the end. Archive modes and owners are
ignored (files 0644, folders 0755). Downloads of several items are packed
into a temporary archive in the agent's protected store and deleted after
the download or within an hour. The preview pane renders Markdown with a
small built-in renderer that produces React elements (no HTML, links only
for http(s) and mailto), pretty JSON and hex dumps; **Edit** opens the
CodeMirror editor. Syntax highlighting covers about 45 languages, each
loaded on first use; `@codemirror/lang-markdown` and `@codemirror/lang-php`
(MIT) were added, the rest come from `@codemirror/legacy-modes`.
**Consequences.** Compress and extract run as one bounded file operation (up
to 10 minutes) rather than a cancellable background job. Windows ACLs
cannot be edited yet. PDF previews are not shown (the console's CSP forbids
framing blobs); PDFs download.

## D-055 Passkeys with an in-house WebAuthn verifier
**Context.** D-006 left passkeys open: `webauthn-rs` needs OpenSSL, and
Forge allows only `ring` (D-005). The owner chose an in-house verifier,
knowing that a hand-written WebAuthn verifier and CBOR parser is
security-critical code.
**Decision.** `forge-identity` verifies passkeys itself (`webauthn.rs`,
`cbor.rs`, `passkeys.rs`) with `ring`; no new dependency. Forge is the
relying party for the host of `public_url` (the RP ID) and its origin. It
checks everything the spec requires of a relying party: client data type,
a single-use challenge (32 random bytes, stored only as a SHA-256 hash with
its purpose, user and a 5-minute expiry, deleted when used), exact origin,
no cross-origin, the RP ID hash, user presence (UP) and user verification
(UV, always required, so a passkey counts as MFA), attested credential data
at registration, the signature, signature counter regression (a counter of
0 means the authenticator does not count) and the credential belonging to
the user. Only public keys are stored. The CBOR reader accepts a strict
subset (integers, byte and text strings, arrays, maps, booleans, null;
definite lengths only; no tags, floats or duplicate keys), bounds strings
to 16 KiB, collections to 64 entries and nesting to 8, rejects trailing
bytes, and bounds-checks every read; deterministic fuzz-style tests run
tens of thousands of random and mutated inputs through the CBOR reader and
the authenticator-data parser. Scope: attestation format `none` only, and
ES256, EdDSA and RS256 only. Recovery: an administrator with `user.manage`
and an MFA session removes a user's passkeys
(`DELETE /api/v1/users/{id}/passkeys`, `auth.passkey.remove_all`), and
`forge-server reset-mfa <login>` on the server host removes passkeys and the
authenticator app and signs out every session (`auth.mfa.reset`).
**Consequences.** Passkeys need the console on HTTPS (or `localhost`) at
`public_url`; changing its host invalidates every passkey. Without
attestation, Forge cannot restrict which authenticator makes or models
users register. Other algorithms and attestation formats are refused. The
verifier and parser need security review whenever they change.

## D-056 Namespaces live in clusters; projects are removed
**Context.** Projects grouped namespaces for ownership, but operators think
in clusters: a namespace's applications run on one cluster, and asking for
a project first added a step with no use. The owner chose to remove projects
completely and to create every namespace inside a cluster, with names
unique per cluster.
**Decision.** There is no `Project` entity: not in `forge-core`, the Cedar
schema, the API, the UI or role scopes. `Namespace.cluster_id` is required
for new namespaces (`POST /api/v1/namespaces`); `PUT /namespaces/{id}/cluster`
moves one and refuses a name the target cluster already has (409); a cluster
that still has namespaces cannot be deleted (409). In Cedar a namespace's
parent is its workspace (`Namespace in [Workspace]`): the cluster is
*placement*, not an authorization parent, so a role bound on a cluster
covers its nodes but not the namespaces placed in it. Making the cluster a
parent would have silently widened every existing cluster binding. The same
review found that `user.manage` did not apply to `Cluster`, so cluster
scoped roles could not be granted at all; it now does.
Migration 0008 is portable SQL. SQLite cannot drop the NOT NULL foreign key
`namespaces.project_id` without rebuilding a table that four others
reference, so the `projects` table becomes the internal `namespace_scopes`
table and the column `scope_id`: a namespace's scope id is its cluster id
(or its workspace id while unassigned), which turns the existing
`UNIQUE (scope_id, name)` into "unique per cluster". Nothing outside the
store reads it. On upgrade, each project binding becomes the same role on
each of that project's namespaces, tenant bindings get explicit bindings on
the namespaces they reached through the tenant's projects, later duplicates
inside one cluster get a short id suffix, project jobs move to their
workspace, and namespaces that never had a cluster stay unassigned until
moved. Converted binding ids are version-8 UUIDs built from the random parts
of the binding and namespace ids, so they cannot collide with generated ids.
**Consequences.** Access after the upgrade is equal or narrower, never wider
(`policy_matrix` and `migration_0008_removes_projects_without_widening`).
Old audit entries keep their `project:` text. Admins should review the
converted bindings (docs: reference/upgrade-notes). The `namespace_scopes`
table stays until a release can afford a table rebuild.

## D-045 Remote access through WireGuard access gateways
*Superseded by D-100: WireGuard remote access was removed; service-scoped
port tunnels are the only way in.*

**Context.** Issue #3 asks for temporary access from a person's laptop or
phone to approved private endpoints in a cluster, with the stock WireGuard
client, expiry, revocation and audit. Making devices fabric members would
give every device the whole mesh and tie revocation to the fabric's
reconcile loop.
**Decision.** Each cluster may name one Linux node as its primary access
gateway and optionally another as standby. A gateway runs a dedicated
WireGuard interface `fgacc<first 6 characters of the cluster id>` with its own key pair (the
private key never leaves the node), separate from the fabric interface.
Devices get addresses from a per-cluster client subnet (a /24 from
`10.250.0.0/16`, checked against every Forge network and other clusters'
client subnets, never carved out of existing leases). Approved traffic is
source-NATed to the gateway's fabric address (destinations see the
gateway), and only client-initiated connections work. Enforcement is a
Forge-owned nftables table pair (`forge_acc_*`, `ip` and `ip6`), validated
with `nft -c` and replaced atomically: default deny for anything from the
interface on input and forward, one accept per device /32 and granted
destination/protocol/port, no client-to-client traffic, return traffic
only to current clients, and no IPv6 at all. Peers are bound to their /32
in AllowedIPs; firewall changes apply before peer changes, and connection
tracking entries of removed or changed devices are deleted, so revocation
also ends established flows. Authorization: `network.connect` (MFA) to
issue or renew one's own profile, `network.access` on each destination
node (or the cluster for administrator-approved CIDRs), `network.manage`
(MFA) for gateways, others' profiles and bulk revoke; console, deploy or
DBA rights never imply network access. A profile's effective access is the
intersection of its selected destinations and the owner's current grants,
re-checked on every sync; namespace destinations expand to the nodes that
host the namespace, and hosts the owner may not reach are reported, not
granted. Expiry is enforced on the gateway: every sync (each minute, and
at once after an issue, renewal or revocation) renews a 5-minute
authorization lease (at most 15) and carries each device's absolute
deadline; a systemd timer on the gateway re-applies the policy every 15
seconds and removes access at the earlier deadline, so a gateway cut off
from Forge fails closed within the lease. Devices register only a public
key; the browser mode generates the key pair with WebCrypto X25519 and
builds the config and QR code locally, so no private key reaches Forge,
its logs or the audit trail.
**Consequences.** A stock WireGuard config is a bearer credential: MFA
guards issue and renewal, not each connection. Standby gives manual
failover through a second, independent profile; there is no automatic
failover or shared endpoint. Devices need UDP reachability to the
gateway; difficult networks and a Tailscale/Headscale subnet-router
connector are later work. Preserving client source addresses would need
a routed mode with return routes in the fabric, also later. Removing a
user's access takes effect at the next sync (within about a minute).
Every read-modify-write of a cluster's remote-access rows (the API and the
sync) runs under a per-cluster lock. The sync releases the lock while it
waits for a gateway, then applies the report to the rows as they are now,
so a revoke or issue made meanwhile is never overwritten. The lock is
in-process: running several server replicas against one database would
need versioned rows instead.

## D-058 Compose stacks run Docker's own Compose plugin; YAML parsed on the server only
**Decision.** Issue #7 adds stacks: a compose document in a namespace, kept
as revisions (SHA-256, author, note), with a sealed `.env` (vault envelope,
names only in clear) and a folder per node (`/opt/forge/stacks/<ns>/<stack>`
or one inside an approved file root). **Create container** produces a
one-service stack from a form, so both share checks, jobs and revisions.
Placement comes from `x-forge.node`, `x-forge.cluster` or Swarm-style
`node.hostname ==` constraints; the control plane splits the document per
node (services plus the networks, volumes, configs and secrets they use) and
refuses `depends_on` across nodes. Each node part goes in a signed
`StackAction` (proto tag 31, allowlist `stack`) as a JSON tree; the root
helper runs `docker compose` with a fixed argv, a cleared environment, and
a compose file it writes itself from the tree (our own YAML writer:
double-quoted JSON scalars), already interpolated with the stack's `.env`
and with `$` escaped, so Compose runs exactly what was checked. `COMPOSE_*`
and `DOCKER_*` keys are refused in `.env`. Deploys are `stack.action` jobs,
one per node (dispatch → await_result → record), idempotent and cancellable;
secrets are unsealed only into the signed command, never into job payloads.
The YAML parser is `serde-saphyr` 2.1.1 (MIT OR Apache-2.0,
`#![forbid(unsafe_code)]`, no OpenSSL, about 0.5 MB, server only): it
supports merge keys and enforces a budget, which we tighten (one document,
depth 32, 50,000 nodes, 2,000 aliases, 512 KiB input), and reads YAML 1.2
scalars (`strict_booleans`, leading-zero numbers as strings). `saphyr` and
`yaml-rust2` lack merge keys and overflowed on deep nesting; `serde_yaml` is
deprecated and `serde_yml` has RUSTSEC-2025-0068.
**Consequences.** Linux nodes with the Compose v2 plugin (the Docker Engine
recipe installs it) only; no `build:`; plain Compose per node, no Swarm. The
file on the node is generated, so hand edits there are overwritten on the
next deploy. Migration 0011 adds `stacks`, `stack_revisions` and
`stack_nodes`.

## D-059 Stack policy: refuse, or require an MFA-guarded admin permission
**Decision.** Every node part is checked twice with the same code
(`forge_catalog::stacks::check`): on the server before saving or deploying,
and in the helper before running Compose, where bind sources are also
re-checked after resolving symlinks. Three outcomes: allowed; needs
`stack.privileged` (privileged mode, host network/pid/ipc/uts/userns/cgroup,
`cap_add`, devices, GPUs, sysctls, `cgroup_parent`, other runtimes,
`security_opt` that disables seccomp/AppArmor/labels, `volumes_from` another
container, the Docker or containerd socket, and binds, bind-backed volumes,
configs or secrets outside the stack folder and the node's approved writable
file roots or on system paths); never allowed (`build`, `extends`,
`include`, `..`/`~` paths, `env_file` outside the stack folder, unknown
top-level keys). `stack.manage` (namespace; admin, operator, developer, like
`deploy`) covers ordinary stacks; `stack.privileged` is admin-only and under
the MFA guardrail, carried into the job payload and re-authorized at every
step. Published ports bind to the node's cluster-network address when its
mesh is up, else `127.0.0.1`; only ports listed in `x-forge.public` bind to
all addresses. Explicit loopback or other addresses are kept.
**Consequences.** A stack using the Docker socket or `/etc` can be saved by
an operator but deployed only by an administrator with MFA; the preview and
the audit record each exception. Approved file roots (D-050) double as the
allowlist for bind mounts. Without a cluster network, services are reachable
only on their own node.

## D-060 Stack services register as applications; exposure reuses Sites
**Decision.** After a deploy, each service on each node is registered as an
application with runtime `compose-service` (spec `{stack, stack_id, service,
image, ports}`) and an instance on that node whose state comes from
`docker compose ps`. The applications API refuses changes to such
applications (`409`), so the stack stays the single source. **Expose via
Nginx** creates a Site whose target is that application: the existing router
resolves it to the node's cluster address, which is where its ports bind.
Without a cluster network the target is `127.0.0.1:<port>` and the only edge
allowed is the service's own node. `forge.expose: http|https` marks services
for exposure in the console.
**Consequences.** Stack services appear in the namespace's services, in
routing and in the Sites target picker without new tables. Removing a
stack's part on a node removes its instances, and applications without
instances are deleted.

## D-061 MCP is served by forge-server over Streamable HTTP, with curated tools
**Decision.** Forge serves the Model Context Protocol itself at `POST /mcp`
(GitHub issue #6), so AI agents (Claude, ChatGPT, Gemini, Codex, Cursor)
connect as connectors without a local process. The transport is MCP
Streamable HTTP in its stateless, JSON-only form: one JSON-RPC 2.0 message
per `POST`, no batches, no SSE stream and no session (`GET` and `DELETE`
answer `405`); protocol versions `2025-06-18`, `2025-03-26` and
`2024-11-05`. JSON-RPC is hand-rolled on axum and serde_json; no MCP SDK
(`rmcp`) or other dependency was added. Tools are a curated catalog
(`api::mcp::TOOLS`) of thin wrappers, each mapping to one REST route; a
tool call builds the REST request from the arguments and dispatches it in
process through the same `/api/v1` router, carrying only the connector's
token and an `McpDispatch` request extension. A test checks every tool
against the build-time route inventory. Results are cut at 64 KiB; calls
are limited per connection (60 per minute, burst 20, 4 at once) and
requests to 1 MiB. `resources/*` and `prompts/list` return empty lists.
**Consequences.** There is one path from `/mcp` to data: the REST handlers,
with their Cedar checks, MFA guardrail and audit, so MCP cannot drift from
the console's authorization. The catalog covers reads and a few reversible
changes; tools are not yet generated from typed OpenAPI schemas (#5), and
streaming, progress and cancellation would need the SSE half of the
transport. Rate limits are in memory per process.

## D-062 Connector identity: dynamic registration, console consent, audience-bound tokens
**Decision.** Connectors use Forge's OAuth 2.1 server with the MCP
authorization profile: protected resource metadata (RFC 9728) at
`/.well-known/oauth-protected-resource[/mcp]` and in `WWW-Authenticate` on
`401`, authorization server metadata (RFC 8414), and anonymous dynamic
client registration (RFC 7591) for public clients only, with strict
redirect URIs (`https`, loopback `http`, or a private-use app scheme; no
wildcards, fragments, user info or dangerous schemes) and a global hourly
cap (`[mcp] registrations_per_hour`, default 30). `/oauth/authorize` sends a
registered connector's request to the console consent page
(`/connect?request=<id>`, in memory, 10 minutes), which only the person's
own browser session can answer. Approval creates an `agent_connections` row;
the code and the tokens carry its `connection_id` and are bound to the
resource `<public_url>/mcp` (RFC 8707). `auth.rs` refuses a connector token
on any request that lacks the in-process `McpDispatch` marker, and `/mcp`
refuses API keys, app tokens and browser sessions. Revoking a connection or
a client ends its tokens at once. Migration 0013 adds `oauth_clients`,
`agent_connections` and `connection_id` on `sessions` and `oauth_codes`.
The `[mcp]` section is off by default.
**Consequences.** Registration is open to anyone who reaches Forge but
grants nothing without a person's approval; the consent page shows the
redirect host because any client can choose its name. A leaked connector
token cannot drive the REST API, and an API key cannot pose as an agent.
There is no allowlist-only mode for pre-registered clients yet. Cloud
connectors need Forge on public HTTPS at `public_url`.

## D-063 Agent modes are enforced on the server; agents never hold MFA
**Decision.** Each connection has a mode chosen on the consent page and
checked by the server on every call: `read_only` (default; read tools only)
or `auto` (also reversible writes within the person's permissions).
`[mcp] max_mode` caps it server-wide. Destructive tools
(`forge_deploy_stack`) and tools whose route's Cedar action is under the
MFA guardrail are never listed or run in any mode. Connector sessions are
issued without MFA, and `auth.rs` clears the MFA flag on every connector
request, even with `dev_skip_mfa`, so any MFA-guarded check reached at run
time is refused and a person must act in the console. Audit events from a
connector's calls keep the person as actor and add `via` (connector,
client id, connection, mode).
File tools refuse the whole-node root, so keys and system secrets on a
node never reach a model provider.
**Consequences.** Client-side permission settings stay a convenience, not
the boundary. Agents cannot run commands, write files or deploy. The **Ask**
mode, in which a change becomes a pending approval a person confirms in the
console, is deferred to the approvals of issue #5; until then such actions
are refused, not queued. Caps per role or workspace are not implemented.

## D-064 MFA is the operator's choice per installation; self-hosted login challenge
**Decision.** Whether MFA-guarded actions need MFA is set per installation
in `[security] require_mfa` (default `true`, env `FORGE_REQUIRE_MFA`), in
any environment. The installer takes `--mfa required|off` (new installs
write `require_mfa = true`; upgrades keep the setting) and
`forge-server mfa status|on|off` edits `forge.toml` in place (comments,
owner and mode kept, atomic replace); a restart applies it. With MFA off,
browser sessions and API keys count as MFA-verified and node revoke asks for
no code; connector sessions still never do (D-063). Running without MFA
shows a console banner (capability `security.mfa_required`), logs a
warning at start, an error in `production`, and a changed setting is
audited as `config.mfa` {old, new} by `system:forge`. `dev_skip_mfa` (and
`--dev-skip-mfa`) stays as a deprecated alias of `require_mfa = false`, and
`dev.skip_mfa` as its inverse capability. This supersedes D-026 and the
later rule that `dev_skip_mfa` was allowed only in the `development` and
`test` environments.
Password sign-in also needs a proof-of-work login challenge
(`[security] login_challenge`, default `true`): `GET /auth/challenge` hands
out a single-use salt valid for 2 minutes, and the client finds a nonce so
SHA-256(salt ‖ nonce) has `difficulty` leading zero bits (17, plus one per
recent failed sign-in for that name, at most +7, reset on success or after
15 minutes without a failure). The server checks one hash; the console
solves it in a Web Worker. Passkeys and SSO are not gated.
**Consequences.** Installations differ in risk: an internal Forge for a
small team may run on passwords alone, a client-facing one should not; the
operator decides, and the choice is visible and audited rather than hidden
behind an environment name. The challenge needs no third-party CAPTCHA, so
it works on-premises and offline, sends no data elsewhere and uses only
`ring` (through `forge-secrets`). It slows password guessing across many
names and addresses, which the per-name throttle alone does not; it does
not stop a determined attacker with many CPUs, so MFA stays the
recommendation. Scripts that sign in with a password must solve it; API
keys need no challenge. Challenges live in memory, so a restart drops
outstanding ones.

## D-065 Custom Nginx blocks use a directive allowlist and need server.command
**Decision.** Site and proxy-profile custom blocks are parsed with an
Nginx-style tokenizer (quotes, escapes, comments, `${}`) and every directive
must be on a fixed allowlist in `forge_catalog::sites`. Directives that take
a file path, change the Nginx process or load code (`root`, `alias`,
`*_log`, `log_format`, `include`, `ssl_*` files, `*_by_lua*`, `js_*`,
`perl*`, `load_module`, `user`, `env`) and Unix sockets are refused; inside
`map`, `geo` and `split_clients` the entries are data. Saving or applying a
non-empty block also needs `server.command` (MFA) on every edge, checked by
the API and again by the job runner.
**Consequences.** A substring denylist let `access_log` write into
`/etc/cron.d` as root; the allowlist closes that class instead of single
directives. Saved blocks with other directives fail on their next apply
(upgrade note). Static files are served by applications, not by the edge.
Let's Encrypt renewals of a site with a custom block need `server.command`
of the last applier.

## D-066 Uploaded certificate keys need certificate.use and go only to the site's edges
**Decision.** A new Cedar action `certificate.use` (platform and workspace
admins, MFA guardrail, on `Workspace` and `Server`) is required to deploy an
uploaded certificate's private key. The sites flow checks it on the
workspace; a site apply with `tls: uploaded` requires it on each edge in the
job runner, and the key is unsealed only for jobs the sites flow created for
the site's current edges. `POST /servers/{id}/commands` refuses
`tls: "uploaded"`.
**Consequences.** `proxy.manage` on one node no longer pulls a workspace's
wildcard key onto it. Operators can still deploy Let's Encrypt sites but not
uploaded certificates.

## D-067 The forge CLI ships in every release as raw binaries per target
**Decision.** Each release publishes the `forge` CLI (crate `forge-cli`) as
raw binaries named `forge-cli-<rust target>[.exe]`, the pattern of
`forge-server-*` and `forge-plugin-runner-*`:
`forge-cli-x86_64-unknown-linux-musl` and
`forge-cli-aarch64-unknown-linux-musl` (static musl, built in the existing
Linux release jobs), `forge-cli-x86_64-pc-windows-msvc.exe` (the existing
Windows job), and `forge-cli-aarch64-apple-darwin` and
`forge-cli-x86_64-apple-darwin`. macOS gets two separate binaries, not one
universal (`lipo`) file, both built in one job on an Apple-silicon runner
(`macos-14`); they need macOS 11 or later and are not code-signed or
notarised yet. Every CLI binary is listed in `SHA256SUMS`, which the release
key signs as before. The CLI is built in its own `cargo build` after the
server and agents, so its features (the `zbus` blocking API the keyring
needs) do not change those binaries. TLS stays rustls with `ring`; no
OpenSSL on any platform.
**Consequences.** The names are a public contract like the other assets.
They never start with `forge-agent-`, because the self-updater copies every
such asset to the server's agent downloads. Linux and Windows release jobs
take a few minutes longer; macOS costs one job at the 10x minute rate.
macOS downloads through a browser are quarantined by Gatekeeper until the
binaries are notarised (follow-up: Developer ID signing with the
certificate held only in CI secrets).

## D-068 The root helper verifies Forge's signature itself
**Decision.** The agent forwards each privileged command to
`forge-agent-helper` exactly as Forge signed it (`{"op":"signed"}`). The
helper checks the Ed25519 signature against Forge's command key and the
target against this node's id, both pinned in its root-only folder
`/var/lib/forge-agent-helper/pin.json`; it checks lifetime and clock skew
(as the agent does) and refuses replays, keeping recent operation ids in
`seen.json` so a restart does not reopen them. It then derives the request
from the command with the same translation the agent uses
(`HelperRequest::from_command`); the agent no longer chooses what runs as
root. Once a pin exists the helper refuses unsigned privileged requests; the
agent's own confirmation after a self-update is the one unsigned request.
Two read-only lookups the agent needs on its own were added later: login
accounts (D-078) and `container_state` for health checks (D-085), which
accepts only `forge-app-*` names and returns only `{id, running, status,
health}`, never the environment, labels or mounts. Full container
`inspect` and every other container operation still need a signed command.
The installer pins as root right after a fresh enrollment
(`forge-agent-helper pin`). A node enrolled before this is pinned on first
use, when the new helper first starts, from the agent's `identity.json`.
**Consequences.** Code running as the `forge-agent` user can no longer act
as root: it can only pass on what Forge signed for this node. First-use
pinning trusts a file the agent user owns, so a node whose agent user was
already compromised before the update is not protected by it; revoking the
node and onboarding it again pins a fresh enrollment as root. The wire
protocol between server and agent is unchanged. A new agent talking to an
old helper falls back to plain requests only when that helper does not know
signed requests, which means it has no pin; a pinned helper never accepts
them. Windows agents run privileged work in their own LocalSystem process,
so this boundary does not apply there.
**Update (issue #15).** Rewriting and fsyncing the whole `seen.json` for
every command cost about 10 ms each and capped file transfers. The helper
now appends each admitted id to `seen.log` (one line, `O_APPEND`, no fsync)
and compacts into `seen.json` (with fsync) every 1024 ids; on start it
merges both, drops expired ids, keeps each id once and the newest 4096, and
compacts a torn or unreadable log at once. A failed compaction keeps
appending. A log over 16 MiB is read from its last 16 MiB (from the first
whole line), so the newest ids are restored, and is then compacted. An append survives a helper restart; only a power loss could
drop the last ids, of commands that expire within minutes. Reviewed by the
security session.

## D-069 Browser terminal certificates are bound to one node
**Decision.** Forge signs terminal certificates whose only principal is
`forge.<server_id>.<login>` for nodes that report node-bound certificates in
their facts (`ssh_principals`), and the bare login only for nodes that do
not yet. On a Linux node where the browser terminal is enabled, the root
helper writes the node's id (from its pin, D-068) to `/etc/ssh/forge_node_id`
and adds `AuthorizedPrincipalsCommand /usr/local/bin/forge-agent-helper
ssh-principals %u` (run as `nobody`) to Forge's block of the sshd
configuration, checks it with `sshd -t` and reloads sshd; the command prints
the one principal the node accepts for that user.
**Consequences.** A certificate for node A is refused on node B, so a terminal
redirected to another node, or a stolen certificate, works nowhere else.
Existing nodes switch at their first agent update; until then they accept
bare-login certificates as before. A node that falsely reported no support
would only weaken its own certificates. `AuthorizedPrincipalsCommand` applies
to every user CA sshd trusts on that node, so a node that also trusts the
operator's own CA refuses that CA's certificates afterwards; this is
documented. Windows nodes keep bare-login certificates.

## D-070 Command kinds an operator removed stay removed
**Decision.** `agent.toml` records in `offered_commands` which command kinds
an install or update has already offered. Re-running the node installer
calls `forge-agent offer-commands`, which adds only kinds this release offers
for the first time; a kind that was offered and is missing from
`allowed_commands` was removed by the operator and stays removed. The
installer no longer edits `agent.toml` with `sed`.
**Consequences.** Restricting a node by editing `allowed_commands` survives
updates. A config written before this tracking cannot tell a removal from a
kind never offered, so the first update after this change adds every missing
kind once; operators who had removed kinds remove them again after that
update. Enrollment records the full list as offered. The agent runs the same
step at every start, so a node that updates itself gets new kinds too (a
self-update does not run the installer).

## D-071 Canary rollouts are a durable parent job over ordinary agent updates
**Decision.** `POST /agents/rollouts` starts an `agent.rollout` job with one
step, `roll`. It splits the nodes into batches of 1 to 100; the first batch is
the canary. For each node of a batch it submits an ordinary `agent.update`
job (the same handler, signing and rollback as a single update) and starts
the next batch only when every node of this one is back on the target
version. The first failed or refused node, or a batch that runs over 30
minutes, stops the rollout; later batches are not touched. Each child job's
idempotency key carries the rollout (`agent.update:<node>:<version>:<rollout>`),
so a `roll` step that runs again after a crash finds its own children instead
of starting them twice, and a later rollout can retry a node that failed.
`server.command` (MFA) is checked for every node before the job is queued,
so a rollout never stops halfway on a permission it lacked. A rollout waits
on its children, so it needs `[jobs] concurrency` of at least 2.
Automatic updates (`agents.auto_update`) are off by default. When an
operator turns them on, the server checks every 5 minutes and, inside a
weekly UTC window, starts one rollout per new version as the person who
turned them on (`enabled_by`), re-authorized like any job. Changing them
needs `workspace.manage` and, when the server requires MFA, an MFA-verified
session; the generic settings editor refuses `agents.*` keys so this check
cannot be bypassed. One try per version: after a failure the server does not
start another rollout for that version.
**Consequences.** Rollouts reuse the tested single-update path; there is no
second update mechanism. Acting as a named person keeps automatic rollouts
inside Cedar and the audit trail: if that person loses `server.command` on a
node, the next automatic rollout fails (`agent.auto_update.failed`) instead
of running with more rights than anyone granted. A failed version waits for
a person to look, so a bad release is not pushed again every window. The
window is UTC only; operators convert local maintenance hours themselves.
**Security review update.** Rollouts and the automatic-update setting are
platform-scoped (`system.update` on the platform for the setting; the parent
job lives on the platform, each child under its node's workspace), so
workspace-level rights neither see nor cancel them. One rollout runs at a
time, and one update per node, guarded by a process-wide lock (one control
plane per database); several rollouts could otherwise hold every job worker
while they wait. The person who enabled automatic updates must still exist,
be enabled and have saved the setting with MFA within the last 90 days;
otherwise the automatic run is refused and audited as that person. A failed
start is retried in the next window.

## D-072 CLI device sign-in (RFC 8628) and step-up for bearer sessions (RFC 9470)
**Decision.** `forge auth login --device` uses the OAuth device grant for
machines without a browser. `POST /oauth/device_authorization` accepts only
the first-party client `forge-app`. Pending requests live in memory for 10
minutes; the device code is stored only as a hash. The user code is 8
letters (`XXXX-XXXX`) from an alphabet without vowels and lookalikes
(`BCDFGHJKMNPQRSTVWXZ`). Only a person's console session, with its CSRF
token, can approve or deny a code (not an API key or a bearer session);
wrong codes are limited to 10 per person per 15 minutes. A decision is
single use: the next poll gets the tokens or `access_denied` and the request
is gone. The tokens carry the MFA flag of the approving browser session.
Answers with `step_up_required` carry `WWW-Authenticate: Bearer
error="insufficient_user_authentication"` (RFC 9470), but keep status `403`,
so the console, which treats `401` as signed out, does not sign the person
out. `POST /auth/step-up` with a bearer session returns a new token pair
with MFA and revokes the old session; the new refresh lifetime still counts
from the original sign-in. Connector (MCP) sessions can never step up. The
CLI prompts for the code on a terminal and retries the request once;
`forge auth step-up` does it explicitly.
**Consequences.** Headless machines sign in without copying passwords or
long-lived keys, and approval stays tied to a person who already signed in
(with MFA when they did). A restart of the control plane drops pending
device requests; the user starts again. Step-up cannot extend a session's
life, and an AI connector cannot raise itself to MFA-guarded actions.
**Security review update.** Device tokens never carry MFA, whatever the
approving session had (with `require_mfa` off every session counts as
verified, and a phished approval should not hand out MFA); the CLI steps up
when an action needs it. The console shows the requesting address and
client before approval, each address may hold at most five pending requests
(loopback excepted), a decision is taken once under the lock that checks it,
and token issuance and wrong codes are audited. Passkey step-up replaces a
bearer session like a code step-up does.

## D-073 Ask mode: agent changes become approvals that run as the approving person
**Decision.** Connections get a third mode, `ask`, between `read_only` and
`auto`; it is the consent page's default (capped by `[mcp] max_mode`). Reads
run in every mode. In `read_only` changes are refused. In `ask` every change
(a write or destructive tool, or any tool whose Cedar action is under the MFA
guardrail) becomes a pending approval instead of running. In `auto`
reversible changes run, and destructive and MFA-guarded tools become pending
approvals; before, they were refused. Approvals live in a generic
`approvals` table (migration 0014) with a `kind` column; `mcp.tool` is the
first kind, and the CLI's approvals (#5) will share it. Forge validates the
arguments before queuing and stores the planned REST request (method, URI,
body) and the raw arguments. The call answers with `{approval_id, status:
"pending", url, expires_at}`, not an error; `forge_approval_status` reads
the outcome. Only the connection's owner decides, from a console session
with CSRF; API keys, bearer and connector tokens cannot. When the action is
MFA-guarded and the server requires MFA, approving needs an MFA-verified
session (`step_up_required`). Approving replays the stored request once
through the same `/api/v1` router with the approver's session cookie and
CSRF token, not the agent's token, and the handler's audit entry gets
`via: {approval, connector}`. A conditional update makes decisions single
use; approvals expire after 15 minutes; at most 10 wait per connection.
Audit: `mcp.approval.request` and `mcp.approval.decide`. This supersedes the
part of D-063 that refused destructive and MFA-guarded tools in every mode.
**Consequences.** An agent can propose any change the person could make,
but cannot approve one: approval needs a browser session, and MFA stays
with people because it comes from the approver's session, never the
connector's. Permissions are checked when the change runs, so a person who
lost a permission since the request cannot run it. The person must read the
request, which came from an AI agent and can be the product of prompt
injection; the console shows it as data. There are no notifications yet:
the agent passes on the link. The pending cap and expiry bound how much an
agent can queue, and queued calls do not count toward the tool-call rate
limit.
**Security review update.** Approving checks again that connectors are on,
that the connection and its client are not revoked and that the mode (capped
by `[mcp] max_mode`) still allows the tool; revoking a connection or client
cancels its pending approvals. Expired requests no longer count against a
connection's limit, destructive tools need an MFA-verified approver, and
every refused decision is audited.

## D-074 Idempotency-Key on mutations: stored, sealed outcomes per person
**Decision.** `/api/v1` honours the `Idempotency-Key` request header (IETF
httpapi draft) on `POST`, `PUT`, `PATCH` and `DELETE`, in a middleware in
front of every handler. MCP dispatch and approval replays pass through it but
never carry the header, so connectors get no idempotency. The key is 1 to 255
visible ASCII characters. Its scope is the authenticated principal and the
kind of credential (MFA-verified session, session without MFA, API key): the
same key from another person, or from a weaker credential of the same person,
is a different key, and a response is never replayed across people or to a
credential that did not pass the checks of the original request (security
review). Responses on secret-bearing paths (the list that is never
compressed: `/auth/`, `/oauth/`, `/me/`, `/vault/`, `reveal`, `token` and so
on) are never stored, each person and kind holds at most 1,000 unexpired
keys, and replays are audited as `idempotency.replay`. A deployment is one workspace (D-016), so the principal is the whole
scope. The request fingerprint is SHA-256 over method, path with query, and
body. The first request claims `(principal, key)` as a pending row in
`idempotency_keys` (migration 0015). A unique constraint settles races.
When the handler answers 2xx with JSON or an empty body (at most 256 KiB),
Forge stores the status, content type and body. The body is sealed with the
vault envelope, because some responses carry a one-time secret (API keys,
access keys). A retry with the same key and request gets that response again
with `Idempotent-Replayed: true`. A retry while the first is still running
gets `409 idempotency_in_progress` with `Retry-After: 1`. The same key with a
different request gets `422 idempotency_key_reused`, as the draft says; issue
#5 had proposed `409`. Any other outcome (4xx, 5xx, a step-up demand, a
streamed or larger body) releases the key, so a retry runs again. Keys
expire after 24 hours. A pending row older than 10 minutes counts as
abandoned (crash or restart) and is freed; the request that claimed it can
then neither store its answer over a newer claim nor free it. The CLI sends a random key with
every mutation and reuses it on its refresh, step-up and transport retries.
After a transport error, a `429`, `502`, `503`, `504` or
`idempotency_in_progress`, it retries up to three times with jittered
backoff, honouring `Retry-After`. `forge api call|request --idempotency-key`
sets the key.
**Consequences.** A lost answer no longer means "run it again and hope":
scripts and pipelines retry safely, and a CLI retry cannot create a second
API key or start a second deploy. Only successes are remembered, so a
failure never sticks to a key; the cost is that a request that failed after
a partial effect runs again on retry, as before. Replays skip the handler,
so their only audit entry is `idempotency.replay`. Every mutation with a key costs one
extra session lookup and two small writes. Job-level idempotency keys
(per job kind, such as the agent rollout keys of D-071) stay as they are;
the header is the HTTP-level equivalent for any operation.

## D-075 CLI terminal and exec through the SSH broker, with a revocable session registry
**Decision.** `forge nodes terminal` and `forge nodes exec` use the same SSH
broker as the console terminal: the server checks `terminal.open` (MFA),
signs a five-minute node-bound certificate (D-069), connects with the pinned
host key and relays bytes. The CLI connects to the existing WebSocket route,
and to a new `GET /servers/{id}/exec` route, with its bearer token in the
`Authorization` header, never in the URL. A bearer session skips the CSRF
`auth` frame, which protects cookies only. A request with both a bearer
token and a session cookie is refused, and connector tokens stay refused
outside `/mcp`. Bearer sessions keep the 12-hour cap and the one-minute
recheck of the sign-in session and `terminal.open`. `exec` takes an argument
list that the server quotes for the POSIX shell, or a shell command line
with `--shell`. Stdout and stderr travel apart (a one-byte tag per binary
frame), stdin flows in until `eof`, and the exit status comes last
(`ExecEvent`, a new streaming `RemoteSession::exec_stream` port; the
buffered `exec` with its short timeout stays for the bootstrap job). The
command line is audited (`terminal.exec`, first 1,000 characters). An
in-process registry lists every open terminal and exec session
(`GET /terminal-sessions`). The owner, or anyone with `user.manage`, can end
one within a second (`POST /terminal-sessions/{id}/revoke`, audited as
`terminal.revoke`). The issue's single-use ticket in
`Sec-WebSocket-Protocol` was not needed: a native client sends headers,
unlike a browser. The CLI adds `tokio-tungstenite` (the handshake only; TLS
is Forge's own rustls with ring and the system roots, as in reqwest) and
`crossterm` for raw mode on every OS. On Windows, console key events are
translated to xterm sequences. All are MIT/Apache-2.0, without OpenSSL, and
only in the CLI.
**Consequences.** Shell access from scripts and AI agents that run the CLI
goes through the same authorization, MFA, certificates and audit as the
console, never through stored SSH keys. An exec cannot be replayed
automatically (WebSockets are never retried). The registry lives in one
server process: a restart ends all sessions anyway, but a future
multi-instance control plane needs a shared registry. Commands with secrets
in their arguments land in the audit log, so the docs say to use stdin.

## D-076 `forge update`: signed self-update of the CLI, verified like the server's
**Decision.** `forge update` reads the same release feed as the server's
self-update (D-037). It downloads `SHA256SUMS` and `SHA256SUMS.sig` and
checks the Ed25519 signature with the release key compiled into the CLI. It
then downloads `forge-cli-<target>` and checks its hash against the signed
sums. The verification code (release key, signature check, sums parser) now
lives in `forge_sdk::release`, which the server's updater re-exports, so
there is one key and one verifier. The CLI writes the new binary next to
itself, runs it once (`--version` must print the release's version) and
swaps it in by renaming. That also works for a running `.exe` on Windows,
and the old binary stays as `forge.prev`. Feeds and downloads must use
HTTPS (loopback HTTP for test feeds). Redirects are followed only to HTTPS,
because GitHub serves assets from its storage host, and no credentials are
ever sent. A newer release installs after confirmation; an older one needs
`--allow-downgrade`. `FORGE_UPDATE_FEED` and `FORGE_UPDATE_PUBLIC_KEY` serve
mirrors and test feeds, as `[update]` does for the server. The CLI's version
now comes from `FORGE_BUILD_VERSION`, like the server and agent; the macOS
release job sets it too.
**Consequences.** A CLI installed by hand stays current without a package
manager, and a tampered download, a wrong signature or a binary that does not
start leaves the installed one untouched. Whoever can set the CLI's
environment can point it at another key; that is the same trust as the
`PATH` that picks the binary. Builds without `FORGE_BUILD_VERSION` report
the crate version, so a local developer build compares as older than any
dated release. Package-manager installs (winget, Homebrew, apt) should
update through their manager; that remains a follow-up.

## D-077 Typed OpenAPI schemas from the handlers' own types; schemars stays in the server
**Decision.** The route scanner (`build_routes.rs`) records each handler's
`Json<T>` body, `Query<T>` and `Json<T>` answer. When the handler's module
defines `T` with `#[derive(JsonSchema)]`, the generated `route_schemas()`
asks `schemars` for its schema. `/api/openapi.json` then lists the types
under `components/schemas` with `$ref`s, expands query types into
parameters and marks each operation `typed`, `partial` or `untyped`.
`schemars` is a dependency of forge-server only. forge-core and
forge-catalog are shared with forge-agent, and every release, CI and lab
build compiles server and agent in one cargo run, so a schemars feature
there would reach the agent's build. Types from other crates are described
in the server: ids as strings and timestamps as integers through
`#[schemars(with = …)]`; four enums (`AgentMode`, `ArchiveFormat`,
`JobStatus`, `RootKind`) and three foreign bodies (`forge_files::NewGrant`,
the two WebAuthn responses) through mirrors in `api/schema_types.rs`. Each
mirror converts from every real variant, or destructures the real struct
without `..`, so a change in the real type is a compile error until the
mirror follows. Larger foreign structs nested in bodies stay free-form JSON.
A gate test fails when a JSON request body has no schema. Its allow list
holds only `POST /sites/preview` (`SiteSpec`) and `PUT
/clusters/{id}/profile` (`ClusterProfile`): large forge-catalog types the
agent shares, documented on the REST reference page. The CLI checks bodies
against the server's own schema before sending (`forge api call|request`
and the typed `--file` commands; `--no-validate` skips it). It uses a small
validator for the subset schemars emits, not a general JSON Schema crate.
`forge api describe` prints the schemas. Unlike the issue's plan, no client
is generated: the hand-written typed commands and the parity gate already
catch drift, and a generator would need another dependency decision.
**Consequences.** Scripts, AI agents and other clients get precise field
lists and early, field-level errors without reading Forge's source. A new
handler with an untyped JSON body fails the gate test until it derives
`JsonSchema`. Most answers are still `Json<Value>` and stay untyped; typing
them is a follow-up that converts handlers to typed answer structs.

## D-078 Terminal accounts: discovered by the helper, created as root, saved on enroll
**Context.** The browser terminal needs an account that exists on the node,
but nodes came up with none that Forge knew about, so operators typed logins
from memory.
**Decision.** Nodes report the accounts a terminal can log in as in their
facts (`login_accounts`): `root` when `sshd -T` allows root logins with keys,
and local users with uid ≥ 1000 (not `65534` or `forge-agent`), a real login
shell and an account that is neither locked nor expired. The root helper
reads `/etc/shadow` and `sshd -T` for this; the request
(`login_accounts`) is read-only, so it is allowed unsigned, like
`confirm_update`. Without the helper the agent falls back to `/etc/passwd`
alone (no `root`, no lock check). `GET /servers/{id}/logins` returns `saved`
and `discovered`; `logins` stays equal to `saved` for older clients.
Accounts are created in two places only: by the installer as root
(`forge-agent-helper terminal-account <name> [--sudo]`, before it enrolls,
never failing the install), and by the signed command kind
`terminal_account` (`POST /servers/{id}/terminal-account`, `server.command`
with MFA, Linux only, audited as `server.terminal_account.create`). The
account gets a home directory, its own group and the password `*`. `*` is
"no password", not `!` ("locked"), so certificate logins also work where
sshd does not use PAM. Passwordless sudo is written only when asked, to
`/etc/sudoers.d/forge-<name>` with a marker line, through a dotted draft
file that `visudo -c` checks before it is renamed into place; asking without
sudo removes the file only when it carries the marker. Names follow the
portable `useradd` subset `[a-z_][a-z0-9_-]{0,31}`; reserved system names,
`systemd-*`, existing system users (uid < 1000) and existing groups are
refused, and an existing regular user is kept as it is. The agent reports
the created account at enroll (`EnrollRequest.terminal_account`), and Forge
saves it as the token's creator (`source: installer`); assisted onboarding
saves the SSH user the same way (`source: onboarding`). Both reuse the
existing saved-logins table, so there is no migration. `terminal_account` is
in the agent's default allowlist and is offered on update (D-070). The
console's **Add node** turns **Create a terminal account** on by default with
`forge-ops` and **Allow sudo** off; the API creates none unless asked.
**Consequences.** Operators get a working login on every new node without
handing out passwords, and the picker shows what each node really has. Each
default account is one more login surface; it accepts only Forge's
five-minute node-bound certificates and has root only when someone chose
sudo. A re-run of the installer on an enrolled node creates the account but
cannot save it (no enroll), so it shows as discovered until someone saves it.
Forge never deletes accounts; removing one stays a manual step on the node.
Windows nodes report no accounts and cannot create them yet.

## D-079 Windows nodes: install.ps1 and a ForgeAgent service, LocalSystem for now
**Decision.** Windows nodes get the same one-line install as Linux. The
server serves the release's `forge-agent-windows-x86_64.exe` at
`/agent/download/windows/x86_64/forge-agent` and a PowerShell installer at
`GET /install.ps1`. The installer is rendered with the server URL, the agent
URL and the agent's SHA-256, never the token. The script, PowerShell 5.1
compatible, requires Administrator and 64-bit Windows 10 1809 / Server 2019
or later. It downloads the agent over HTTPS (plain HTTP only with
`FORGE_ALLOW_HTTP` for labs) and checks its SHA-256, then installs it to
`C:\Program Files\OrbynForge`. It restricts `C:\ProgramData\OrbynForge` to
SYSTEM and Administrators, enrolls with `$env:FORGE_ENROLL_TOKEN` (removed
afterwards) and runs `forge-agent service install`. Re-running upgrades in
place and keeps the identity; `-Uninstall [-Purge]` removes it. It changes
nothing else on the machine: no firewall rules (the agent only connects
out), no execution policy, no PATH. The agent gains `forge-agent service
install|uninstall|run` on the `windows-service` crate (MIT/Apache-2.0, about
29 KB, built on the windows-sys the agent already uses, so the documented
`unsafe` exception does not grow). The `ForgeAgent` service starts
automatically, the SCM restarts it after 5 s, 10 s and 60 s on failure, and
it logs to `<state>\logs\forge-agent.log`. The console's Windows command
reads the token masked and pipes `install.ps1` into `iex`. The service runs
as **LocalSystem**. A stop reports Stop pending at once (PowerShell's
`Stop-Service` gives up after about 2 s on a service that still reads
Running), cancels the heartbeat in flight, and is bounded: after 15 s the
service reports Stopped and the process exits. `install.ps1` stops it with
`sc.exe stop`, and after 30 s clears the failure actions and ends only its
own `forge-agent.exe`, so an upgrade or uninstall never leaves the service
running.
**Consequences.** Windows nodes are installed, verified and kept running
like Linux nodes, without copying binaries by hand. LocalSystem keeps the
trust boundary of the old manual `run` as Administrator, but a compromised
agent process is SYSTEM on that node, where Linux confines the agent to an
unprivileged user and a small privileged helper. The owner later accepted LocalSystem
as the design for the agent itself rather than a pending risk: a split into
a virtual-account agent and a LocalSystem helper would add a service and a
named-pipe protocol without removing the need for LocalSystem, while the
agent opens no port, verifies every signed command and `allowed_commands`
narrows it per node. The security roadmap lists it as Accepted. Apps and
services Forge installs on Windows run under an account the operator
chooses instead (D-081).
Self-update and rollouts still skip Windows nodes, and assisted (SSH)
onboarding stays Linux only.
**Hardening.** Nothing that runs as LocalSystem passes through a folder
other accounts can write. `install.ps1` downloads to
`C:\Program Files\OrbynForge\forge-agent.exe.download`, checks it there and
moves it over `forge-agent.exe`; `%TEMP%` is not used, because a program the
same user runs could swap the file between the checksum and the copy. Any
user can create folders in `C:\ProgramData`, so the installer keeps an
already locked `OrbynForge` folder, and otherwise refuses one that holds a
link or another account's item instead of taking it over. It then sets the
owner on the folder itself and a fresh protected ACL (SYSTEM and
Administrators) with `Set-Acl`, not `icacls /grant:r`, which keeps other
entries, and checks again afterwards. `forge-agent service install` refuses
a binary outside `%ProgramFiles%`. The agent itself locks the data folder
at `enroll`, `service install` (both fail if it cannot) and each service
start (a warning), so a hand-run `enroll` never writes the private key into
a folder that inherits ProgramData's Users read access. The rendered script
no longer puts the server URL into its header comment.

## D-080 Process actions on Linux and Windows nodes, without new unsafe code

**Context.** The node process view (htop-like panel, `forge nodes
processes`) was read-only, and on Windows it showed raw SIDs, a load
average of zeros and `?` states. Operators need to end a runaway process
or lower its priority on both OSes, from the console and the CLI, with the
same authorization, MFA and audit as other root-equivalent node actions.

**Decision.** A new Cedar action `process.manage` (platform admin,
workspace admin, operator; in the MFA guardrail) gates
`POST /servers/{id}/processes/{pid}/signal` and `…/priority`. Both are
audited as `server.process.signal` / `server.process.priority`, also when
the node refuses. They travel as the signed command `ProcessAction` (tag 33,
allow-list kind `process_action`). The request names the process's start
time from the list; the node refuses when the PID now belongs to another
process, and refuses protected processes for the whole tree before acting:
PID 0, Forge's own components (`forge`, `forge-*`), on Linux init and
kernel threads, on Windows PID 4 and the session and security processes
(`smss`, `csrss`, `wininit`, `services`, `lsass`, `LsaIso`, `winlogon` and
the kernel's pseudo-processes).

Linux: the root helper runs the action, derived only from the signed
command (D-068). Signals (`term`, `kill`, `hup`, `int`) go through a pidfd
opened before the start-time check, so a reused PID never receives them;
`nice` (-20 to 19) through `setpriority`. All via rustix, no `unsafe`.

Windows: by the owner's choice, no new `unsafe` code. The agent
(LocalSystem) runs the built-in `%SystemRoot%\System32\taskkill.exe`
(End task without `/F`, End process with `/F`; trees are walked by Forge so
every member is checked) and the built-in Windows PowerShell for priority
classes (`idle` to `high`, never `realtime`), both by absolute path with a
fixed argument list carrying only integers and a class name from a closed
set. Owner names come from sysinfo's account list plus the well-known
service SIDs; the listing reports `load: null` and no state letters.

**Consequences.** One permission, one audit trail and one console panel
for both OSes; the console adapts its columns and menu to `summary.os`
and the `actions` the node reports, so older agents show no menu.
Limits on Windows: owners that are neither local nor well-known (domain
accounts) show as SIDs; there is no per-process priority column; a
priority change takes about half a second (PowerShell start-up); End task
reaches only processes with a window in the service's session, so desktop
apps and console programs answer that they can only be ended forcefully;
and between the start-time check and `taskkill` a PID could in theory be
reused (no process handle without `unsafe`), a window of milliseconds that
Linux closes with the pidfd.


## D-081 Each native app runs as its own readable account (Linux); Windows workload accounts planned

**Context.** Native apps ran as a system account `fa-<app22>-<hash>`: one
per app, but unreadable in `ps`, `ls -l` and audit records, created without
any mark, and found again only by recomputing the name. The owner wants every
app and service Forge runs (not Docker containers) under its own account
named after the app, for auditability and to confine its files (and later
the file explorer and SFTP) to it. The main platform is Ubuntu and other
Linux distributions; Windows stays available with basic features, untested.

**Decision.** The account is `<namespace>-<app>`, which already fits user
name rules (slugs start with a letter and use `a-z`, `0-9`, `-`). When that
is longer than 32 characters, or an account Forge did not create already has
it, Forge uses the name shortened to 25 characters plus a 6-hex hash of
`<namespace>-<app>`. The GECOS comment `Orbyn Forge app <ns>/<app>` marks the
account as Forge's; the agent adopts an existing account only with exactly
that mark, so it never takes over a system or human account, and refuses with
a clear message when both names belong to foreign accounts. Accounts are
looked up through NSS (`getent`) and managed with `useradd`, `usermod` and
`groupmod` long options, with the `nologin` path detected (Debian/Ubuntu
`/usr/sbin`, RHEL-likes `/sbin`), so it works across distributions. The home
stays `/srv/forge/apps/<ns>/<app>` (0750, owned by the account); the env file
is `root:<account>` 0640 in a namespace folder that is traversable but not
listable (0711). An account an older Forge created is renamed on the next
deploy with `usermod --login` (and its group with `groupmod --new-name`)
after the unit is stopped; the uid stays, so every file the app owns stays
its own. The node reports the account in the deploy result and the job audits
it. This is `run_as: dedicated` of the D-081 design; choosing other accounts
(root, an existing user) and the Windows side (virtual service accounts,
Local/Network Service, vault users, a service host for non-service binaries,
PostgreSQL's service account) are designed but parked on the roadmap, not
built.

**Consequences.** An app's processes, files and audit entries carry its name.
A node with a clashing local account still works, with the hashed name.
Removing an app keeps its account and files, as before. The rename needs the
app's processes stopped, which the deploy does for its own service; a stray
process under the old account makes the deploy fail with `usermod`'s message.

## D-082 Plain deploy stays without MFA; container mounts outside the app folder need stack.privileged
**Decision.** `deploy` stays outside the MFA guardrail. With MFA required,
API keys and connector sessions never count as MFA-verified (D-063, D-064),
so guarding `deploy` would end CI pipelines and connector deploys; with MFA
off it would change nothing. `deploy` is the unprivileged path instead:
native apps run as their own system user, and anything that reaches root is
guarded separately. Container app volumes follow the compose-stack rule: a
plain deploy may mount only the app's folder
`/srv/forge/apps/<ns>/<app>/` and the node's approved file roots; any other
host path needs `stack.privileged` (MFA). The API checks each node before
any job starts, puts the mounts in the job payload (`privileged_mounts`),
and the job then re-authorizes `stack.privileged` at every step and refuses
a mount that is not in the payload (an edit after submission). Docker
sockets, system paths and Forge's folders stay refused for everyone.
**Consequences.** Before this, a Developer or an API key could mount, for
example, `/var/spool/cron`, `/var/lib/dpkg`, `/home` or another namespace's
app folder writable into a root container: root on the node without MFA.
Existing containers keep running; redeploying one with such a mount needs an
administrator with MFA (upgrade notes). A per-namespace "deploys need
approval" setting is on the security roadmap as Planned.

## D-083 File access on a native app's folder runs as the app's account (Linux)

**Context.** The file explorer and the SFTP gateway reach an app's folder
through the root helper, so only Forge's path checks kept an approved
application root inside it, and every file it created was root's. With each
native app running as its own account (D-081), the kernel can enforce that
boundary instead.

**Decision.** On Linux an approved root that is exactly an app's folder,
`/srv/forge/apps/<ns>/<app>`, is a `RootSpec::App { namespace, app }`;
roots approved earlier on such a folder are converted when they are used.
A plain directory that holds app folders (`/srv`, `/srv/forge`,
`/srv/forge/apps`, `/srv/forge/apps/<ns>`) or lies inside one is refused,
by the control plane and again by the node, so nothing bypasses the rule.
The root helper resolves the app's account itself (D-081's mark and shape
checks; never a name or uid from the request), refuses owner changes, the
root-owned `.forge` launcher folder and moving or removing the folder
itself, strips setuid and setgid from `chmod`, and starts its own binary
by absolute path as `forge-agent-helper file-worker` with the app's uid and
gid (std `CommandExt`, which drops supplementary groups; no `unsafe`), a
cleared environment and cwd `/`. The worker checks in `/proc/self/status`
that every user and group id is the app's, lowers its limits with
`setrlimit` (no core dumps, 512 files, CPU time, 2 GiB file size) and runs
the unchanged file engine on the folder as a plain directory. Trash and
revisions go to `/var/lib/forge-app-store/<ns>/<app>` (0700, the app's,
under root-owned 0711 parents; protected; removed with the app only when it
is a real folder of the app's account), not into the folder, which apps
often serve. `<ns>/<app>`, not `<ns>-<app>`: slugs contain `-`, so two apps
could otherwise share a store. File access requires the app's folder to
exist as a real directory of its account. The helper treats the worker as
untrusted: strict answer schema, 48 MiB cap, 120 s timeout (600 s for
search), at most 8 workers per node, and it never acts as root on anything
the worker returns. File audit events on an app root add `app` (from the
control plane) and `as_reported` (from the node). Cedar actions and MFA are
unchanged. Links are listed and never followed.

**Consequences.** A bug in Forge's path checks on an app root reaches no
more than the app's account could; other apps' folders, `/etc/forge` and
Forge's stores stay out of reach by kernel permissions. New files belong to
the app. Files root created there before stay root's and are read-only
through the explorer (no recursive chown; a later "fix ownership" action
would lchown root-owned entries only). The app's own processes share the
worker's account and, where `kernel.yama.ptrace_scope` is 0 (Debian's
default), could attach to it; the hardening guide recommends 1. Each
operation starts one process; SFTP keeps one per operation for now, a
worker per session can follow if uploads are too slow.

**Phase 2 (Landlock).** Before it starts any thread, the worker confines
itself with Landlock: read and write only beneath the app's folder and its
store, plus read-only `/etc/passwd` and `/etc/group` for owner names; no
TCP bind or connect; no abstract UNIX sockets or signals outside the
sandbox. Filesystem rights up to ABI v5 (truncate, device ioctls), best
effort, so kernels without Landlock (RHEL 8) or with an older version run
with what they have; moves between folders (`REFER`, the trash) need v2.
The worker reports the level it reached (`full`, `partial`, `none`, or
`error: <reason>` such as a container's seccomp filter); the helper logs a
warning with it once, and the file audit records it as
`confined_reported`. On `error` the worker carries on (fail open): failing
closed would break the file explorer in containers where the app account's
permissions still hold. Dependency audit: `landlock` 0.4.7 (MIT OR
Apache-2.0, maintained by the Landlock LSM project, safe API over the
syscalls, so no `unsafe` in Forge code), Linux-only in `forge-agent`; it
adds `enumflags2`/`enumflags2_derive` (MIT OR Apache-2.0, already in the
workspace through `forge-cli`) and well under 1 MB; no OpenSSL;
RUSTSEC-2023-0035 does not apply to the pinned `enumflags2` 0.7.12; `cargo
deny` passes. Within the dependency policy (permissive, small, no
OpenSSL), so recorded here rather than asked. Hand-written syscalls were
rejected because they would need `unsafe` in Forge.

## D-084 Fleet-wide local names in every node's hosts file
**Decision.** Every enrolled node, in a cluster or not, keeps the names of
its workspace's nodes and services in a marked block of its hosts file
(`# BEGIN orbyn-forge names` … `# END orbyn-forge names`), and in the
hosts files of its Forge containers. Names follow the existing structure
with the suffix `[names] suffix` (default `local`):
`<node>.<cluster>.local` and `<app>.<ns>.<cluster>.local` in a cluster,
`<node>.local` and `<app>.<ns>.local` outside one. A node's label is the
one a user sets (`PUT /servers/{id}/dns-name`, Cedar `server.rename`,
audited `server.dns_rename`) or one derived from its console name or
hostname when the node first gets a label (`-<id>` added when taken). Both
kinds are stored in `node_names` (migration 0017, unique per workspace) and
never move on their own: renaming one node never changes another node's
name, a console rename does not change a stored label, and a name freed by
a rename stays free until a user sets it. New fabrics default to the
domain `<cluster>.<suffix>`, so their mesh block and the names block agree.

**Sync.** The server rebuilds every node's list every 30 s and right after
a rename or enrollment, and sends a signed `names` command (tag 34,
`system:names`) on the node's next heartbeat when its list changed, when
it reconnects and every 10 minutes (repairing hand edits and restarted
containers). Addresses: the mesh address when both nodes are in the same
fabric, otherwise the address the server sees the node's agent connect
from. Never a node's own claim: hostnames only feed derived labels after
`label_from`, and ports come from the stored app specs. Ports cannot live
in a hosts file; `GET /names`, `forge dns list` and the console's **Local
names** panel show them. SRV records through the agent-embedded resolver
were not built; that is a separate proposal.

**Safety.** Names are built only from validated labels (`[a-z0-9-]`,
1–63, no leading or trailing `-`, ≤253 in total, reserved labels such as
`localhost` refused); IPs are parsed addresses written with `Display`, one
entry per line. The node checks the list again: the suffix must pass
`check_suffix` (one special-use label such as `local`, `internal`, `lan`, or
a domain of two or more labels the operator owns) and every name must end
in it, so a bad list cannot put `github.com` into a hosts file. Only the
marked block changes; every other byte stays. With damaged markers nothing
is deleted and a fresh block is appended once. Files are rewritten in place
(same inode, under one lock with the fabric's block, `O_NOFOLLOW`, fsync):
`/etc/hosts` may be bind-mounted into containers, and Docker bind-mounts
each container's own hosts file, so an atomic rename would leave them
stale. In place also keeps owner, mode and SELinux context. A file over
16 MiB (4 MiB for a container) is refused, never cut. The write is not
atomic: a crash between truncating and writing can leave a hosts file empty
or partial; the next sync writes Forge's block again, but the rest of the
file cannot be rebuilt. A label another node holds (set or derived) is
refused with `409`, so a user who may rename one node cannot take another's
name. Two renames racing between the check and the save are an accepted,
low risk; the unique index still stops a duplicate.

**`.local` and mDNS.** The user asked for `.local`. It is also the mDNS
domain (RFC 6762). On Debian, Ubuntu and RHEL-likes the hosts entries win
only while `/etc/nsswitch.conf` lists `files` before `mdns4_minimal
[NOTFOUND=return]` (the default). Programs that ask mDNS directly (Avahi
tools, some browsers, macOS clients on the LAN) do not read the hosts file,
and other LAN hosts can still answer `*.local` over mDNS; Avahi on a node may
announce `<hostname>.local` itself. Operators who want to avoid this set
`[names] suffix` to `internal`, `lan`, `home.arpa` or a domain they own.

**Transition.** Existing fabrics keep their stored domain
(`<slug>.forge.internal`) and its block, so their old names keep resolving
until the operator changes the domain; the new `.local` names are added
next to them. No forced migration.

**Tests.** Unit tests for labels, blocks (byte-for-byte, CRLF, damaged
markers, injection), size limits, suffix checks, renames that take a name,
and sync timing; an `it` test with agents that refuse the command (this
machine's hosts file is never written), plus a lab end-to-end run.


## D-085 Observations: one shared record of health evidence, with an ordered transition feed

**Context.** Health checks (#12) produce evidence that alerts (#11),
automation (#8), observability (#14) and database probes (#9) all need. If
each consumer ran or stored checks itself, one failure could trigger three
copies of the same action, and stale data would be read as current.

**Decision.** `forge_core::observation` defines the vocabulary and pure
rules: an `ObservationKey` (resource, the node of an instance, a validated
lowercase dotted `Subject`), state values (`HealthState`, with
`StartupState` kept separate), `#[non_exhaustive]` reason codes, the source,
a workload `generation`, and the functions `decide`, `stale_transition` and
`clamp_observed_at`. Four small ports split the access: `ObservationStore`
(write, used only by the service), `ObservationQuery` (read),
`TransitionFeed` (ordered, `seq` cursor kept by each consumer) and
`ObservationRetention` (delete). The new `forge-health` service
(`ObservationRecorder`) owns validation, the per-node reporting rule,
change detection and the write policy. The store only applies a batch
atomically.

- **What a node may report.** Only `server:<itself>` (with no instance node
  or itself), and `application:<id>` with `server = itself` where an
  instance of that app is placed on it (checked through the
  `InstancePlacements` port, implemented by the store). Other servers,
  clusters, unplaced apps and every other kind are dropped and counted. The
  node is recorded as the executor.

- **Freshness is on the server clock.** `fresh_until = received_at +
  fresh_for_ms` (clamped to 1 s to 1 h). A stale value reads as `unknown`;
  the last known value stays visible. Executor times more than 30 s off are
  clamped and the row is flagged `skewed`, never rejected.
- **Unknown is not evidence.** It is neither failure nor recovery. A
  different generation always starts fresh.
- **Writes on change.** The current value is rewritten only when it changes
  or its freshness falls below half. History gets a row per change plus one
  keep-alive row every 5 minutes, and is kept 7 days. That keeps 1,000 nodes
  with several checks each within what SQLite handles.
- **Transitions are an outbox in the same transaction.** `seq` is
  `MAX(seq) + 1` inside the transaction, because one migration set serves
  SQLite and PostgreSQL and they have no common identity syntax. The primary
  key turns a concurrent duplicate into a conflict instead of a silent
  reorder. A unique dedupe key (key, generation, from, to, evidence time)
  makes a replayed batch add nothing. Retention keeps the newest 100,000
  transitions and never deletes the newest row, so `seq` never restarts.
- **Authorization.** New action `health.view`, granted to every role that
  has `view`, on `Platform`, `Workspace`, `Namespace`, `Cluster`, `Server`
  and the new `Application` entity (parents `Namespace`, `Workspace`;
  resolved through the application's namespace). Observations are not
  audited: they are evidence, not changes. Health policy changes will be
  audited when they arrive.
- **The transition feed has no workspace column.** Every consumer (#11
  alerts, a future feed API) must authorize or filter each transition by
  its resource's workspace itself; the feed is not safe to expose as is.
- **Deleted resources.** The periodic sweep removes current values and
  history of servers and applications that no longer exist. Transitions
  stay as the record of what happened.
- **Follow-ups.** History pages by `since` only; add a `before` cursor when
  the UI pages through long histories. A resource that doesn't exist and
  one the caller can't see both answer `403` on the read API (no existence
  leak).

**Alternatives.** Persisting the in-memory node telemetry (`metrics.rs`) as
observations was rejected for v1: it is a live view by design, and numeric
measurements can come later as a new `ObservedValue` variant. Detecting
transitions inside the store was rejected: it is a domain rule (rule 1).

## D-086 Access profiles and service accounts (issue #13, phase 1)
**Context.** Seven fixed roles cannot express "view the UAT nodes, restart
one application, no shell". Background work (schedules, CI, MCP) needs an
identity that is not a person. The enforcement inventory
(`plans/authz-inventory/`) is the baseline this builds on.

**Decision.**
- **Cedar stays the only engine.** An access profile is a named, explicit
  list of actions. Each change publishes a new immutable revision. Each
  revision becomes a Cedar *template* built in Cedar's JSON form from typed
  `Action` values only: `permit(principal == ?principal, action in [...],
  resource in ?resource)`. An assignment links that template with the user
  and the scope as slots. No name, description or other user text ever
  reaches policy text; scopes come from an allow-list (workspace, cluster,
  namespace, server, application) with real ids, and stored action lists
  are re-parsed on load.
- **Built-in roles are unchanged.** Role bindings stay template-linked as
  before. Built-in profiles are generated from the schema's role groups
  (`Schema::action_entities`), so they cannot drift; a parity test shows a
  role binding and an assignment of its built-in profile decide the same for
  every action, resource kind and MFA state. Built-in profiles are
  read-only; duplicating one makes an editable custom profile.
- **No hidden growth.** A custom profile never gains actions added to Forge
  later; "select all" means the actions listed at that time.
- **Revisions.** An assignment follows its profile's latest revision;
  publishing relinks every assignment at once. The revision in force is
  recorded in the audit entry of every grant and publish. A publish checks
  the expected current revision, so a concurrent edit conflicts instead of
  overwriting.
- **Revocation.** Deleting an assignment, or archiving or deleting a
  profile, unlinks at once. Expiry is checked by the engine at every
  decision (an expired link is dropped before evaluating); a sweeper removes
  the rows every 30 seconds and logs failures.
- **No races.** Assigning, publishing and archiving run one at a time
  (one server-wide lock: these are rare administrative changes), and an
  assignment is stored only while its profile is unarchived and still at the
  revision that was checked (`INSERT … WHERE EXISTS`, fails closed), so it can
  never stay linked to an older, wider revision or an archived profile.
- **Escalation guard.** Assigning needs `user.manage` on the scope, an
  MFA-verified session, and the assigner must hold every action of the
  profile on that scope. Publishing a revision runs the same check on every
  scope the profile is assigned at, against the *publisher's* own rights
  there; a dry run returns the impact. MFA is checked before that, so a
  session without it is asked to step up first. Revoking needs no MFA. Managing profiles and service accounts is the new
  `access.manage` action (administrators, MFA).
- **Service accounts** are users with `kind = 'service'`: no password,
  passkey, authenticator or SSO, never an interactive sign-in. They use
  tokens (API key rows on that user: hashed, 90-day default expiry,
  last-used; at most 10 per account and 20 issued per hour) and never hold MFA.
  Every interactive sign-in (password, SSO, passkey) refuses a service
  account explicitly. Disabling one revokes all its tokens, ends its
  sessions and closes its terminals and event streams at once; enabling it
  again does not restore tokens (issue new ones). The user and its
  service-account record are created in one transaction. The owner is
  accountable but lends no access; a disabled owner is flagged, not
  cascaded.
- **One workspace.** Scopes are inside the deployment's one workspace
  (D-016). If hosting several workspaces is ever wanted, it is a workspace
  per hostname that supersedes D-016, never a per-request selector. Not
  planned.

- **Migration 0019** was numbered before 0020 to 0031 were released; sqlx
  applies any migration a database lacks, so existing installs (the lab,
  early releases) get it on the next start. A store test proves it.

**Alternatives.** Generating policy source as text was rejected (injection
risk, re-validation of everything). A separate `ServiceAccount` Cedar
principal type was rejected for phase 1: it would touch every `appliesTo`;
service accounts are users with a kind. Profile inheritance and explicit
denies were left out: several assignments combine as a union, and the
guardrails stay the only forbids.
**Amendment: health policies (#12 slice 2).** An application's startup,
readiness and liveness checks are one typed `forge_catalog::health::HealthPolicy`,
stored in the application spec as `health` and shipped to the node with it.
The console's form and YAML view, the API, the CLI and (slice 3) the node's
executor all use this one model; the server's `validate()` is authoritative
and unknown fields are refused. Probes are HTTP(S), TCP, exec (argv, never a
shell) and shell (explicit `sh`/`bash`).

- **Targets (SSRF).** HTTP and TCP probe the workload itself unless
  `allow_remote_host` is set. Link-local and metadata addresses and names
  are always refused, and so are other nodes of the fleet (host name, `.local`
  name, fabric address). Numeric host spellings (`2852039166`, `0xa9fea9fe`,
  octal) are refused, and IPv4-mapped, IPv4-compatible and NAT64 addresses
  are checked as the IPv4 they embed. At probe time (slice 3) the node
  resolves once, checks every resolved address (forbidden, and fleet by IP),
  connects to the checked address without resolving again, and treats a
  redirect as a new target (re-checked, or not followed).
- **Commands (invariant).** Exec and shell checks run ONLY as the app's own
  account (D-081), confined by Landlock like the file worker (D-083), or
  inside the app's container: never as root, the agent's user or the
  helper's. That is why `health.command` stays with Developer: developers
  already deploy code that runs as that account, so a probe adds no
  privilege. The executor (slice 3) enforces this with tests, kills on
  timeout and caps output; where it can't guarantee the confinement (for
  example on Windows) it refuses to run them and reports
  `unsupported_executor`. Saving one needs `health.command` (MFA guardrail);
  other changes need `health.configure` (the roles that have `deploy`).
- **Secrets.** Credential headers take only `vault:<id>` references, resolved
  on the node at probe time. Saving a reference needs the same right as
  revealing the item (platform administrator, `secret.read` with MFA, no API
  key), the item must exist, and the reference id is audited; the executor
  checks again before it sends it. The audit entry (`application.health.update`)
  lists purpose, name, type and target per check, never scripts or header
  values.
- **Endpoints.** The policy has its own `GET`/`PUT /applications/{id}/health`
  (optimistic `expected_updated_at`) and a `validate` endpoint for the
  editor, so an ordinary application update can't drop it. The container's
  Docker `HEALTHCHECK` stays separate and is not converted; the editor offers
  to import it.
- **Default checks.** `default_checks` (on unless turned off) gives every
  service a liveness check with no configuration: native, the systemd unit is
  active plus TCP to the app's declared port; containers, running plus the
  image `HEALTHCHECK`, else TCP on the published port. Readiness defaults to
  liveness until a readiness check is configured. 10 s period, 3 failures,
  freshness 3× the period. A node that goes offline makes its keys unknown at
  once, not only after freshness runs out (slice 3).

**Amendment (#11 slice 2, D-087): numeric measurements.** `ObservedValue`
gains `Measure { milli: Option<i64>, unit: Unit }` (`Unit`: percent, bytes,
seconds, count; `None` is the unknown form, so `unknown()` and Eq/Hash still
hold). A measurement changes on almost every sample, so within one
generation a measure-to-measure change is **not** a feed transition; only
going unknown or coming back is. Threshold rules read `latest`, not the
feed. The recorder writes a measurement only when it moves past a deadband
(±1.0 for percentages, ±1 % of the previous value otherwise) or its
freshness falls below half, which keeps 1,000 nodes reporting every 25 s
well under SQLite's write budget. A moved measurement updates the current value
only; history gets measurements at keep-alives (every 5 minutes) and on
transitions. The server records one measurement batch per node at most
every 10 s, whatever heartbeat pace the agent chooses. The first producer is the server itself:
each heartbeat's telemetry becomes `node.cpu.percent`,
`node.mem.used_percent` and `node.disk.used_percent` (root filesystem) on
`server:<that node>`, through `record_from_agent` (the node reports about
itself), fresh for three heartbeat holds. No wire change. Subject families
are listed in `KNOWN_SUBJECT_PREFIXES` (`health.`, `node.`). Reviewed with
#12's owner.

**Amendment: running checks on nodes (#12 slice 3b).**

- **Where.** The agent schedules checks and runs the read-only ones itself:
  systemd `ActiveState`/`InvocationID` over D-Bus, TCP connects, and container
  state through the helper's `container_state` lookup, the one unsigned
  container request (D-068): `forge-app-*` names only, four fields only. When it relays a deploy it saves a
  descriptor of the app under its state directory (namespace, name, runtime,
  `listen_port`, whether defaults are on and readiness is configured). The
  descriptor never holds the environment; the full spec stays root-only.
- **Reporting.** Results ride on the heartbeat (`Heartbeat.health`, field 8,
  at most 256 per beat): changes plus a keep-alive per check at half its
  freshness (90 s, so a report waiting for the next long poll is still
  fresh). The hub maps `(namespace, app)` to the application placed on the
  reporting node, drops the rest, and records through `record_from_agent`.
- **Offline means unknown at once.** When the offline monitor marks a node
  offline, `ObservationRecorder::expire_node` writes `unknown` with reason
  `node_offline` for everything about the node and its instances, with
  transitions, instead of waiting for freshness to run out.
- **Instance status.** A pure rule in `forge-health` (`instance_status`):
  unhealthy if startup is exhausted or liveness or a required readiness check
  fails; unknown if anything required isn't passing yet or is stale; degraded
  if only an optional readiness check fails; healthy otherwise.
  `GET /applications/{id}/health` returns it per node, with each check, plus
  the effective default checks in their resolved form (`default: true`).
- **Duplicate ports.** Two native services on one node that declare the same
  `listen_port` are ambiguous: the node checks only the unit for them (reason
  `ambiguous_port`), the API returns an `ambiguous_listen_port` warning, and
  nothing may be wired to that port automatically. Deploy is not blocked.
- **Later slices.** Configured HTTP/TCP checks and policy pushes without a
  redeploy are in D-105. Still to do: vault secrets fetched per probe on the
  authority of the person who last saved the policy, re-checked with their
  current roles each time and audited (switchable to a service account once
  #13 lands); exec and shell in a confined helper `probe-worker`.
## D-087 Alerts: a reusable rule engine, durable alarm episodes, evaluation separate from notification
**Context.** Issue #11 asks for alarms on nodes, services and other
observations, with exact time semantics, and the same rule evaluation is
needed by automation event rules (#8) and node alarms (#14).

**Decision.**
- **`forge-rules` crate.** Pure evaluation with no Forge types and no
  clock. A `RuleSpec` has a trigger condition (threshold, state in, state
  not in), a mode (immediate, sustained, time-weighted window average with
  a coverage minimum, repeated distinct breaches with a minimum spacing,
  any in window) and a recovery (its own condition for hysteresis, plus a
  hold time). A `Tracker` per series takes samples in time order. Late or
  duplicate samples, and samples more than 30 s in the future, are
  refused. Every sample counts only until its `valid_until`; time without
  valid evidence is unknown. Unknown interrupts a pending condition and a
  pending recovery but never resolves a firing alarm. Trackers serialize,
  so a restarted evaluator keeps its timers.
- **Alarms.** `forge_core::alerts` holds rules (with a revision), alarm
  episodes, their timeline and two ports (`AlertRepository`,
  `AlertStateRepository`). Migration 0020 stores them. A partial unique
  index (`fingerprint WHERE state = 'firing'`) gives exactly one active
  episode per series (rule, resource, node, subject); a recurrence is a new
  episode linked to the last one. An alarm ends without recovery when its
  rule is deleted, disabled or changed (a new revision never mixes window
  histories), or when its resource is deleted. Acknowledgement and notes
  are separate from the state.
- **Evaluator** (`forge-alerts`). Every 15 s the server runs one pass. For
  each enabled rule it reads the subject's current evidence from a
  `SeriesSource`, resolves each resource's workspace through
  `ResourceResolver::resolve_many`, as D-085 requires, and keeps only the
  rule's workspace and scope. Unknown or deleted resources are dropped.
  The first source is the control plane itself (`node.agent.status`);
  observations from nodes join through the same trait (agreed with #12:
  numeric values as `ObservedValue::Measure`, read from `latest`, not
  from the feed).
- **Evaluation never sends anything.** Notifications (a later slice) read
  alarm transitions through an outbox, so a failed message can neither
  rerun a remediation nor erase or reopen an alarm.
- **Authorization.** `alert.view` (all roles but customer) and
  `alert.manage` (platform and workspace admins, operators) on the
  workspace. Alarm lists are also filtered to resources the caller may
  `view`; a hidden alarm answers `404`. Rule changes, acknowledgements and
  notes are audited; evaluator passes are not (the alarm timeline records
  them).

**Slices.** 1: rules, alarms, node agent status. 2: numeric measures
(CPU, memory, root disk) as observations from heartbeat telemetry, read by
the evaluator through `ObservationQuery::latest` (see the D-085
amendment). 3: channels, policies and delivery with an
outbox and SSRF-safe webhooks (migration 0026 or 0027). 4: Handlebars
templates (a new dependency, audited first), silences.

**Alternatives.** Evaluating inside the observation recorder was rejected:
it would tie alarm state to the write path of health evidence. Driving
evaluation only from the transition feed was rejected for timing rules: a
sustained or window condition needs time to pass without a transition, so
the evaluator reads current evidence on a timer.

## D-091 Port tunnels: a relay through the control plane, dialled back by the node
**Context.** Issue #35: forward a local port to a port on a node from the
CLI, like `ssh -L` (for example `psql` to PostgreSQL on a node), including
nodes behind NAT. The agent link was request/response only (heartbeat
long-poll, signed commands, 4–8 MB bodies), and the browser terminal goes
server to node over SSH (D-069), which needs the node's SSH port reachable.

**Decision.** Phase 1 relays every TCP connection through the control
plane, and the node dials back:

1. `forge tunnel open -n <node> -p <port>` listens locally, then
   `POST /api/v1/tunnels` checks `network.tunnel` (new Cedar action, MFA
   guardrail; operators and admins), that the node is online, the port
   allow-list, and the per-user limit. The CLI holds the tunnel with a
   control WebSocket (`/tunnels/{id}/control`). An unclaimed tunnel is
   dropped after 30 s.
2. Each accepted local connection opens `/tunnels/{id}/connect` (a
   WebSocket with the bearer token in the header). The server signs a
   `TunnelOpen` (proto tag 38) with a connection id, the port, the node's
   allow-list, a single-use 32-byte stream token (only its hash is kept),
   the idle timeout and the tunnel's end.
3. The agent checks it (port in the signed allow-list, well-formed ids,
   not expired, under 32 connections), dials `127.0.0.1:<port>` or
   `[::1]:<port>` only (never a host name or another address, so a tunnel
   cannot pivot elsewhere), then opens its own mTLS HTTP/2 connection to the
   hub and posts a streaming request to `/agent/v1/tunnel/<id>` with the
   token. The request body carries bytes from the node; the response body
   carries bytes to it. The hub checks the certificate like every agent
   request and that the stream belongs to that node; the token is single
   use. The server splices the two legs.

Bytes travel raw (no base64, no JSON), on a connection separate from the
heartbeat. Backpressure is end to end: bounded channels (8 frames of
64 KiB) between the legs, HTTP/2 windows of 2–4 MiB, and TCP. In the
integration test 4 MiB crossed both ways through the full path in 0.19 s
on one machine.

**Limits and revocation** (`[tunnels]`, defaults): 8 tunnels per user, 32
connections per node, 256 per server, 15 minutes idle, 12 hours of
lifetime. The control socket rechecks every 15 s that the owner's sign-in
(or API key) still exists and `network.tunnel` still holds; a lost
permission, a revoked session or key, `DELETE /tunnels/{id}`, idleness or
the lifetime closes every leg at once. A token rotation does not: the CLI
sends each refreshed access token on the control socket, and the server
follows it only when it is a live session of the same user with the same
second-factor state. Open, close (with bytes and duration), revoke and
allow-list changes are audited; refusals too. Data and tokens never are.

**Port allow-list.** *Amended by D-100: the hand-typed allow-list and
`PUT /tunnel-ports` are gone. A tunnel names a service from the service port
catalogue (or an SSH or, for administrators, a raw port); access comes from
a role on the service's namespace or an explicit grant; the lifetime is
8 hours by default, at most 7 days, `never` only behind
`[tunnels] allow_never_expiry`; tunnels are rows too.* Originally: a default
list and per-node lists in the settings store (`tunnel.ports`,
`tunnel.ports.<node>`), changed with `workspace.manage`; empty meant no
tunnels.

**Who may tunnel.** People only. AI connectors cannot reach the tunnel
endpoints: their tokens are refused outside `/mcp`, no MCP tool offers
tunnels, and the endpoints refuse connector sessions explicitly. The
control socket needs a bearer session (the CLI); the console lists and
closes tunnels but does not open them. A non-loopback `--bind` needs
`--allow-remote-bind`, prints a warning and is audited: anyone who can
reach that address can use the tunnel. The bind address and the flag are
reported by the CLI; the server records them but cannot verify them.
Services that trust loopback (PostgreSQL `trust`, a Docker TCP socket,
Redis without auth, local admin UIs) are open to anyone allowed to tunnel
to their port, which is why the allow-list starts empty. Following token
refreshes keeps a tunnel open past the session that opened it, bounded by
the tunnel lifetime.

**Phases.** 1b (done in D-100): service targets from the port catalogue.
2: `--direct` through the node's SSH with node-bound certificates (D-069)
for nodes the CLI can reach.

**Alternatives.** Chunked transfers over the command channel, like file
transfers (#15), were rejected: a round trip per chunk makes interactive
protocols slow, and they would take heartbeat command slots. SSH
`direct-tcpip` from the server was rejected for phase 1: it fails behind
NAT, needs sshd forwarding, and Windows nodes rarely run sshd.

## D-092 Remote access redesign: reachable gateway addresses, real network addresses, generated keys
*Superseded by D-100: WireGuard remote access was removed. Agents still
report `interfaces` in their facts; `NEVER_EXPIRES` moved to
`forge_core::clock` and is shared by SFTP grants, tunnels and tunnel grants.*

**Context.** On the lab, a generated profile (D-045) was unusable as is:
`Endpoint = lab-a:51821` named the node's host name, which a device cannot
resolve; `AllowedIPs = 10.64.0.16/32` was a cluster-mesh address that means
nothing to the person reading it; and `PrivateKey` was a placeholder the
person had to fill in. Remote access also lived in two places (Settings and
the cluster's Network page). Issue #3, follow-up comment.

**Decision.**
- **Reachable addresses only.** A gateway has a list of addresses devices
  may dial (`GatewayAddress { endpoint, label, interface }`, 1–8, the first
  is the default). Forge offers the gateway node's IPv4 interface addresses,
  which agents now report in their facts (`interfaces`, through `sysinfo`;
  loopback, Forge tunnels and container bridges left out; an older agent
  offers the address it connects from). An administrator can add an address
  or DNS name by hand (a router's public address with a port forward,
  `vpn.example.com`). Every address must be an IPv4 address or a DNS name
  with a dot, plus a port: `valid_device_endpoint` refuses bare host names.
  A profile stores which address it dials (`access_profiles.endpoint`,
  migration 0030; `NULL` = the first); if the gateway stops offering it the
  profile falls back to the first.
- **Addressing mode per cluster gateway setup.** `real` (default for new
  setups): a node destination resolves to the node's address on a network
  the gateway itself sits on. The address is the one Forge observes (the
  agent connection address the hub sees, else the operator SSH endpoint,
  as in #18); node-reported interface addresses count only before the
  first heartbeat, so a compromised node cannot point devices at another
  LAN host through the NAT (`real_destination`: same subnet as one of the
  gateway's interfaces), so the gateway reaches it directly; the existing
  forward chain allows only the granted address/protocol/port tuples and
  the existing masquerade rule source-NATs only what the forward chain
  accepted. A node that shares no network with the gateway is refused with
  a hint to use `cluster`. `cluster`: the fabric `/32` as before. Settings
  saved before D-092 have no mode and read as `cluster`, and an old
  `endpoint` reads as one address labelled "Configured endpoint", so
  existing gateways and profiles keep working unchanged. The mode cannot
  change while profiles hold addresses (they would point at addresses the
  gateway no longer forwards to). During resyncs, a real-mode member whose
  interfaces are momentarily unknown keeps its last address instead of
  losing access. Gateways stay Linux-only.
- **Split tunnel only.** `AllowedIPs` lists exactly the destination
  networks (a `/32` per node). `client_config` never writes a route
  shorter than `/8`, and rules refuse `/0` anyway.
- **Keys: "Forge generates the keys" without Forge seeing them.** The
  console creates the X25519 pair in the browser (WebCrypto) and merges the
  private key into the configuration locally, for display, QR code and
  download, once; the CLI's `--generate-key` runs the local `wg genkey` /
  `wg pubkey` and writes the file with mode 0600 (never over an existing
  file). The server keeps receiving only the public key. We chose client-
  side generation over a server-generated, return-once key because it keeps
  the D-045 property that no private key ever exists on the control plane,
  in memory, logs or backups, at no extra cost: browser support for X25519
  is current in every major browser, and "Use my own key" remains for the
  rest. No new dependency (the console's `qrcode` package and the CLI's use
  of the system `wg` tool already exist or need nothing new).
- **Destinations by name and a hosts snippet.** Profile responses list what
  the device can reach (name, address, protocol, ports) and a hosts-file
  snippet for the fleet names (`<label>.<suffix>`, D-084) of its node
  destinations. Answering DNS on the gateway is left for later.
- **Never expiry, opt-in.** The expiry choice is 1 h, 1 day, 7 days,
  30 days, Never or a custom number of hours. Never needs
  `[remote_access] allow_never_expiry = true` (default `false`, the same
  name as `[sftp] allow_never_expiry`) and `network.manage` on the cluster,
  which is what `file.admin` is to SFTP. It is stored as
  `NEVER_EXPIRES` (9999-12-31), which fits every timestamp column and the
  gateway's saturating deadline arithmetic, so the gateway, watchdog and
  leases need no change. It is flagged (`never_expires`), audited on issue
  and renew, the denial is audited (`remote_access.never_expiry`), and the
  profile is revocable like any other; the gateway lease still closes
  access when the control plane stops renewing it.
- **Navigation.** Network ▸ Remote access ▸ Gateways (per cluster) and
  Profiles (one per device and person, with a profile page). The cluster
  Network page and Settings link there.
- **CLI parity.** `forge remote-access gateway-set|gateway-off|addresses`
  cover the gateway endpoints that were console-only; `create` gains
  `--generate-key`, `--endpoint`, `--never`, `--gateway`.

No new Cedar action: `network.connect`, `network.access` and
`network.manage` with MFA cover every change, as in D-045.

**Alternatives.** Resolving a node's LAN address from its agent connection
address alone was rejected: it is the address the control plane sees,
which behind NAT is neither on the gateway's network nor the node's own.
Routing real addresses over the fabric from a gateway elsewhere was
rejected for this slice: it would need per-node forwarding and NAT on every
destination; `cluster` mode already covers nodes spread across networks.

## D-093 Object stores: the node is the failure unit; data deletion needs an ownership marker
**Context.** A distributed RustFS install on lab VMs failed because two data
paths of one node shared the root disk. RustFS refuses that by default
("local erasure endpoints must use distinct physical disks"). Separately,
**Remove** gained a delete-data option, and data paths are free-form
directories that could name a home directory, an app's data or another
store's path.
**Decision.** (1) The node is the failure unit. Drives of one node may share
a device. The recipe logs a warning per shared drive and sets
`RUSTFS_UNSAFE_BYPASS_DISK_CHECK=true` only for that member. Drives of
different nodes on one NAS stay the operator's responsibility (documented).
(2) Every data path carries `.forge-objstore` with the store id, written at
install, adopt and status, and only into an empty directory or one holding
RustFS data. Delete-data removes a path only when the marker matches, the
path and its parents are not symlinks or reparse points, it does not overlap
another store's data path on the node (the server sends them), and no other
store's marker lies below it. Otherwise the path is skipped and logged.
(3) The new `owner` and `delete_data` fields are rejected by older agents'
strict parsers. The job resends other operations without `owner`. A delete
fails with "update the agent on <node> to delete data".
**Consequences.** Losing a shared disk loses all of that node's drives at
once; durability then rests on parity across nodes. Data in a directory that
held other files before install is never deleted by Forge. A first install's
marker sits at the root of each drive. It is not a valid bucket name, so
RustFS is expected to ignore it; confirm this on the lab before relying on it.

## D-094 Migrations squashed into one initial migration before the first release

**Decision.** The 20 migrations up to `0031_file_grant_owner.sql` are squashed
into `0001_initial.sql`, which runs their SQL in the original order. The
originals stay in git history.

**Why.** No install has to be upgraded across them: the lab is rebuilt from its
clean checkpoint, and the only published build is the `v0.1.0-rc-01`
pre-release (2026-11-10), which is not in use. One file is easier to read and
review, and new installs run one migration instead of twenty.

**Consequences.**
- A database created by an earlier build (including `v0.1.0-rc-01`) cannot be
  upgraded: sqlx finds applied migrations that no longer exist. Reinstall
  instead.
- From here on the rule applies again: never edit or squash a migration that a
  published build contains; add a new file. The next migration is `0002_…`.
- The two tests that only checked upgrades between old migrations (0008, 0019)
  are removed; their behaviour is covered by the schema itself.

## D-095 Native apps are prepared on placement and get an automatic app root
**Context.** Lab test findings F-03, F-05, F-06: a native app's folder and
account existed only after its first deploy, so code could not be uploaded
first; its app root (D-083) had to be approved by hand per node (default
read-only); and the same folder could be approved twice.

**Decision.**
- New app action `prepare` (`forge_catalog::apps::APP_ACTIONS`): the node
  creates the app's account, folder and file-explorer store, and nothing
  else. Placing a native app on a new Linux node submits an `app.action`
  `prepare` job. No new Cedar action: `prepare` is part of `deploy`, which
  the job re-authorizes at every step.
- After a successful `prepare` or `deploy`, the job approves a writable
  `application` root (`RootSpec::App`, `application_id` set) unless a root
  on that folder exists; `remove`, removing a never-deployed placement and
  deleting the application remove it. Both are audited (`file.root.create` /
  `file.root.delete`, `automatic: true`). Rationale: the caller already holds
  `deploy` on the namespace and `server.manage` on the node, and the root
  covers only the app's folder, run as the app's account; using it still
  needs `file.read`/`file.write` on the node. A root approved by hand on the
  folder is kept as it is (including read-only).
- Duplicates: `FileService::add_root` refuses a second root of the same kind
  on the same folder (`409`, naming the existing root). Exact duplicates
  stored earlier are hidden from the root list (oldest kept) rather than
  deleted by a migration: their ids may be referenced by open uploads, SFTP
  grants and scripts, and a code-level filter cannot lose data.

**Alternatives.** A `0002` cleanup migration deleting duplicates was rejected
(irreversible, could orphan grants). Creating the root in the API handler
instead of the job was rejected: the folder exists only once the node has
prepared it.

## D-096 Readable CLI output: colour, key/value views, YAML/XML, paging (MOP E-01–E-04)

**Decision.** `forge` never prints raw JSON as its default human output.
`--output table` renders objects as key/value views (nested sections,
epoch-ms as local date-times) and lists as tables with a page footer;
`-o json` is pretty on a terminal and compact when piped; `-o yaml` and
`-o xml` print the same `forge.cli/v1` envelope from small hand-written
serializers in `forge-cli/src/out.rs` (no YAML/XML crate). Colour only on a
terminal (`--color auto|always|never`, `NO_COLOR`), using plain ANSI codes;
`anstyle-query` (already in the tree through clap) enables VT mode on old
Windows consoles. Long output pages through `$FORGE_PAGER`/`$PAGER`/`less`,
or a small built-in pager on Windows (crossterm, already a dependency).
List commands take `--limit/--page/--all`; a CLI test
(`queryparity.rs`) reuses the server's route scanner and fails when an API
query parameter has no matching flag or a `parity.json → query` mapping.

**Dependencies.** `time`'s `local-offset` feature, read once before the CLI
starts its worker thread; it adds `num_threads` (MIT/Apache-2.0, a few
hundred lines) on Unix only. `anstyle-query` (MIT/Apache-2.0). No new
OpenSSL or non-permissive license; agent binary unaffected.

## D-097 Elevated tokens: per-token MFA powers for automation (amends D-021, D-064, D-086)

**Decision.** API keys and service-account tokens get an `elevated` flag
(`api_keys.elevated`, migration `0002_token_elevated.sql`, `BIGINT` 0/1,
default 0). Where MFA is required, an elevated token counts as MFA-verified
(`mfa = !mfa_required || (elevated && allow_elevated_tokens)`), so CI can run
MFA-guarded actions (exec, packages, updates) without a prompt. Creating one
needs an MFA-verified session (unless MFA is off) and the new
`[security] allow_elevated_tokens` (default `true`, `forge-server
elevated-tokens on|off`, `FORGE_ALLOW_ELEVATED_TOKENS`). With the policy off,
creation is refused (`elevated_tokens_disabled`) and existing elevated tokens
act as normal ones. Tokens never prompt; `factor_verified` stays false, and
every audited call made with a token records `token_id` and `elevated`.
Connector (MCP) tokens never hold MFA (D-063 unchanged).

**Why.** With MFA required, no token could run guarded actions, so the CLI
was unusable in CI. A per-token flag keeps normal keys low-power, makes the
exception visible in lists and the audit log, and lets hardened production
installs turn it off.

## D-098 Version choice for self-update, upgrades only, agents follow, migration guard (amends D-037)

**Decision.** `GET /system/update/releases` lists releases newer than or
equal to the running version (newest first, from the GitHub-compatible list
URL: `[update] releases_url` or `feed_url` minus `/latest`; drafts and
prereleases only for a prerelease server). `POST /system/update/apply
{version, agents, batch}` installs any listed version strictly newer than the
running one, fetched by tag; never a downgrade. The signed pipeline is
unchanged. The requested agent follow-up (`rollout` by default) is stored in
`applied.json` with the applier's principal and context; once the new server
serves and the agents have reconnected, it submits one `agent.rollout`
(D-071) as that person, re-authorized per node, audited as
`system.update.agents`. The dry run also runs the new binary's
`check-migrations`, which exits non-zero when the database has applied
migrations the binary does not know.

**Why.** Operators want to stage upgrades (N+1 before N+2) and keep agents
in step with the server. Downgrades are refused because migrations only move
forward. The migration guard stops the start-up crash loop a binary older
than its database would otherwise cause.

## D-099 `forge-server enroll-token`: local enrollment for the installer

**Decision.** `forge-server enroll-token --cluster <name> --hostname <h>
--ttl-minutes <n>` runs on the server host (local root or the service user),
creates the cluster if missing, files the token under the first enabled
platform administrator (enrollment tokens name a person), audits as
`system:forge`, and prints only the token on stdout (logs go to stderr). The
interactive installer uses it to enroll an agent on the server host.

**Why.** Access to the host and its database already implies full control;
a local command avoids handing the installer an admin password or session.

## D-100 Service-scoped port tunnels replace WireGuard remote access
**Context.** The API port is the only way in: the CLI, console, terminals,
SFTP, agent downloads and tunnels already use it. WireGuard remote access
(D-045, D-092) did not: it opened a UDP port on every gateway node and
needed outbound UDP from the client. Tunnels (D-091) were too loose to carry
that weight: they lived in server memory only, the "allowed ports" were a
hand-typed administrator list with no relation to what listens, anyone with
`network.tunnel` on a node reached any listed port, the agent dialled
loopback only (stack and container ports often bind to the cluster network
address), and the CLI remembered nothing.

**Decision.**
- **Remote access is removed entirely**: the server modules, the
  `remote_access` and `access_profiles` tables (dropped in migration
  `0003_tunnel_grants`), `forge_core::remote_access`,
  `forge_catalog::access`, proto `AccessAction` (tag 23 retired, never
  reused), the agent's gateway code and the helper's `access-watchdog`
  timer, the Cedar actions `network.connect`, `network.access` and
  `network.manage`, the `[remote_access]` config section (still accepted and
  ignored, with a warning), the CLI commands and the console pages. The
  cluster network (fabric, D-030) stays: it is the node mesh. Agents keep
  reporting `interfaces`.
- **Targets are services.** A tunnel names an `app` (native: `listen_port`;
  container: host ports from `spec.ports`), a `stack` service (the deployed
  plan's published ports), an App Store `package` (its `port` parameters,
  else its declared ports) or a node's `ssh` port; a `raw` node port is an
  administrator exception. The **service port catalogue**
  (`forge-server/src/service_ports.rs`, `GET /service-ports`,
  `GET /servers/{id}/service-ports`) is built read-only from existing data
  and carries each port's `dial_ip`: the address the port is bound to on
  the node (`0.0.0.0` reads as loopback). The agent dials that address only
  when it is loopback or one of its own interface addresses (`TunnelOpen`
  gains `dial_ip`, tag 7; older agents ignore it and dial loopback), so a
  tunnel never pivots to another host.
- **Both access paths.** `network.tunnel` (MFA) now applies to a namespace
  or application as well as a server, and developers hold it: a developer,
  operator or administrator on the service's namespace may tunnel to its
  apps and stack services; packages carry no namespace and are checked on
  the node. SSH targets reuse `terminal.open`. New `network.tunnel.any`
  (administrators, MFA) covers raw ports. An administrator
  (`access.manage`, MFA) may also issue an **explicit grant**
  (`tunnel_grants`: user, target, optional node and ports, own expiry,
  revocable) for a person without the role; the grant alone opens the door
  and Cedar is not consulted for the grantee. Raw ports are never granted.
- **Lifetime.** `min(requested, the grant's expiry, [tunnels]
  max_lifetime_hours)`; `[tunnels] default_lifetime_hours = 8`,
  `max_lifetime_hours = 168` (was 12), `never` only with
  `[tunnels] allow_never_expiry` (default off) and `access.manage`. The
  control socket re-checks every 15 s that the sign-in exists and that the
  role or grant still holds; revoking a grant closes the tunnels it allowed
  at once (`access_withdrawn`).
- **Tunnels are rows too** (`tunnels`): history, the console list and
  restart recovery. The live sockets stay in the server's registry; rows
  still open when the server starts are closed as `server_restart`, rows of
  tunnels never claimed as `unclaimed`. Open, close (with bytes, duration
  and reason), revoke, grant create and revoke, and every refusal are
  audited; `raw` tunnels are marked as such.
- **CLI and console.** `forge tunnel open --app|--stack --service|--db|
  --ssh|--port`, `--for 30m|8h|3d|never`, `forge tunnel grants`; remembered
  local ports live in the CLI context file, keyed by target, node and
  remote port (a free port is a property of the machine). The console lists
  tunnels and grants and shows each service's ports; it cannot open a
  tunnel (a browser cannot bind a local port) and says so.

**Alternatives.** Keeping WireGuard behind the API port (a TCP transport,
or a userspace WireGuard in the CLI) was rejected: it would re-implement a
VPN to reach the same ports a tunnel reaches, and leave a second access
path to audit. Making the port allow-list per service (an administrator
types the ports) was rejected: the ports are already known from the specs
and plans, and a typed list drifts. Checking grants through Cedar
(per-user templates) was rejected for this slice: a grant is one row with
its own expiry and revocation, and the SFTP grants (`file_grants`) set the
pattern.

## D-101 Container page: shell inside a container without MFA, environment values with MFA

**Decision.** The container detail page gets two backend additions.
`POST /servers/{id}/containers` with `{"op": "inspect", "reveal_env": true}`
returns the container's environment *values* (`result.env`, `[{key,
value}]`) next to the usual `env_keys`. It needs the new Cedar action
`container.inspect_env` (platform and workspace admins, operators) under the
MFA guardrail, is audited as `container.env.reveal` with the node, the
container id and the number of variables, never the values, and the values
are returned in that call only: the `agent.command` job stores the answer
without `env` and the API hands the values over from memory once. An agent
that does not know `reveal_env` (wire field 6 of `ContainerAction`, ignored
by older agents) answers with names only, and the API then fails with
`agent_update_required`. `GET /servers/{id}/containers/{container_id}/shell`
opens a WebSocket with the node terminal's handshake, frames, session
registry and audit (`container.exec` / `container.exec_close`), connects as
the node's first saved login (the installer's terminal account, D-078; or
`login=`) and runs one fixed command on the PTY, `sudo -n docker exec -it
<id> sh -c 'command -v bash … || exec sh'`, through the new
`RemoteSession::shell_command` (PTY + exec channel; the node shell is
unchanged). It authorizes the new action `container.exec` (same holders),
which is **not** under the MFA guardrail: the operator chose a shell inside a
container to be a routine action, like `container.manage`.

**Why.** Environment values are where container secrets live, so reading them
stays a guarded, audited, non-persisted step. A container shell is confined
by the container, so it does not need the step-up a node shell does; the
caution is that a privileged container, or one with the Docker socket or host
paths mounted, reaches the host, so `container.exec` should be granted like
`terminal.open` on nodes that run such containers. The docker command is
server-built from a validated id: no user input reaches the shell line.

## D-102 Applications group environments (app_groups)

**Context.** Teams run one product in several environments: `shop` has
`test`, `staging` and `uat`, each with the same services. Forge had the
two lower levels, a `Namespace` (one environment in one cluster, D-056) and
an `Application` (one service placed on nodes), but nothing that said
which namespaces are the same product. The console showed a flat list of
namespaces and the operator kept the grouping in the names.

**Decision.** The product model is **Application → Environment →
Service**. A new entity, `app_groups`, is the product; today's `Namespace`
is its environment and today's `Application` row is a service. The
database, the REST paths, the Cedar types and the audit actions keep their
names: `namespaces` stays `namespaces` and `applications` stays
`applications`, and the new table and routes are `app_groups` /
`/app-groups`. The console labels an app group *Application*, a namespace
*Environment* and an application *Service*. Renaming the existing tables
and routes would have changed every client, migration, policy, audit row
and document for a cosmetic gain; the UI label is where the user sees the
word, so that is where it changes.

- `app_groups(id, workspace_id, name, description, created_at)`, name a
  lower-case slug (letters, digits, dashes, 1-63) unique per workspace;
  `namespaces.app_group_id` (nullable, indexed). Migration `0004`.
- `POST /namespaces` **requires** `app_group_id` (400 when missing or
  unknown, same workspace, `workspace.manage` on the group). An environment
  without a product is the state the model removes, so new ones cannot be
  created in it. Namespaces that existed before the migration stay
  **unassigned** (`app_group_id: null`); `PUT /namespaces/{id}/app-group`
  attaches them, and the console offers that for every unassigned
  environment. Nothing is auto-assigned: guessing the product from a name
  would put environments into the wrong product silently.
- `GET /app-groups` returns each group with `environments[]` and a
  `service_count`; `GET/PUT/DELETE /app-groups/{id}` and
  `PUT /namespaces/{id}` (rename) and `DELETE /namespaces/{id}` complete
  the set. A group cannot be deleted while it has environments, and an
  environment cannot be deleted while it has applications or stacks (409):
  the caller sees what is in the way and moves or removes it first.
- Cedar: `AppGroup in [Workspace]`, `Namespace in [AppGroup, Workspace]`.
  The resolver gives a namespace with a group that group as a parent, so a
  role binding scoped `app_group:<id>` covers every environment of the
  product and what runs in it, and nothing else. No new actions: `view`
  and `workspace.manage` apply to the group; everything inside it is
  governed by the actions that already apply to namespaces.
- Audit: `app_group.create|update|delete`, `namespace.rename`,
  `namespace.delete`, `namespace.set_app_group`.
- CLI: `forge namespaces create --app-group <id|name>` (required) and
  `forge namespaces list --app-group`; the app-group routes themselves are
  console actions reachable through `forge api call` (parity class `api`).

**Alternatives.** Making the app group optional on new namespaces was
rejected: optional grouping becomes no grouping. Deriving groups from the
`Application` rows (one product per service name across namespaces) was
rejected: services are named per environment and a product has several.
Renaming `Namespace` to `Environment` across the code base was deferred to
a release with other breaking API changes; the UI label covers the user's
need now.

## D-103 Replica or standby when a service runs on several nodes; ingress from the service page

**Decision.** Placing a service on a second node asks for its role: **Replica**
(active, serves traffic; the default) or **Standby** (Nginx `backup`, used
only when every replica is down). The roles are the existing `active` and
`standby` instance roles (D-020); only the console changed, so the API and
the site resolver are untouched. The service page gains a **Routes** panel
listing the ingress endpoints that target it (filtered from `GET /sites` by
`targets[].app_id`) and **Expose through ingress**, which opens the endpoint
editor pre-filled with the service, its port and the environment's cluster,
plus the endpoint's **Balancing** (`round_robin`, `least_conn`, `ip_hash`
from D-022) when the service has two or more replicas. Applying to edges
stays a separate, MFA-guarded step on the Ingress page. The Routing view
marks each upstream `replica` or `standby`.

**Why.** Running one service on two nodes in the same environment was the
missing step between "a service" and "a highly available service"; the
engine support (multiple upstreams, backups, balancing) already existed and
only needed to be reachable from where the operator places nodes. A
container service also needs Docker on the extra node, so the dialog checks
it before placing.

**Consequences.** Health-check-driven removal of a replica from the
endpoint, upstream weights and per-upstream settings stay on the roadmap;
Nginx `max_fails` is the only failure detection. Stateful services are not
replicated by Forge; the docs say so.

## D-104 Self-update watchdog counts agent restarts; the update job fails on a rollback

**Decision.** Extends D-054 (issue #47). While `update-pending.json` exists,
the helper reads the agent unit's `NRestarts` from systemd every 5 seconds
and rolls back once systemd has restarted the agent `MAX_STARTS` (3) times
since the swap. The marker stores the counter's base (`agent_restarts_base`):
the value at the swap, lowered when the counter restarts from zero, or the
first reading for a marker written by an older helper. The 5-minute deadline
and the helper start count stay. On the control plane, the `agent.update`
job's `await` step fails as soon as the node is online on a version other
than the target with a heartbeat more than 30 seconds after the update was
sent.

**Why.** On the lab, an agent that crashed at start was rolled back only at
the 5-minute deadline, because only helper starts were counted; the job then
waited out its 6-minute limit although the node was already back. The server
stores no agent start time, so the job uses the heartbeat time: the old
agent stops about 3 seconds after accepting the update, so a later heartbeat
comes from an agent started since.

**Consequences.** A crashing agent is restored in well under a minute and
the job reports it at once. An agent that runs but cannot reach the control
plane still waits for the deadline. A node whose old agent took more than 30
seconds to stop would be reported as rolled back while its update goes on;
the node itself is not affected, and a new update request starts a new job.

**Amendment (lab run).** The 30-second assumption was wrong. The restart
goes through `systemd-run --on-active=3`, and a systemd timer fires within
its accuracy window, one minute by default. On the lab the old agent kept
answering for 52 and 62 seconds after it accepted the update, so two of
three successful updates were reported as rolled back. The agent now sets
`AccuracySec=1s` on that timer, and the job waits 120 seconds before it
reads a heartbeat on another version as a rollback, which also covers agents
that still have the old timer.

## D-105 Configured HTTP and TCP health checks run on the nodes; policies are pushed and reconciled by revision

**Decision.** Extends D-085 (issue #12, slice 3b-2).

- **Execution.** The agent runs configured HTTP(S) and TCP checks itself
  (`forge-agent/src/health_probe.rs`), each at its own period, timeout,
  initial delay and thresholds, with at most 8 checks at once. The target
  rules are applied again where the connection is made: an empty host is
  `127.0.0.1`; any other host is resolved once, every address is checked
  (link-local and metadata always refused; loopback only without
  `allow_remote_host`; one forbidden address refuses the whole target) and
  the probe connects to a checked address. Redirects are not followed. The
  request is written by hand (HTTP/1.1, `Connection: close`, status line
  only), with header and path values that contain a line break refused.
  HTTPS verifies against the machine's trust store
  (`rustls-native-certs`, already in the tree) unless the check turns
  verification off.
- **Startup.** A startup check runs until the first success (`succeeded`)
  or `failure_threshold` failures (`exhausted`), once per workload
  generation; readiness and liveness report `pending` until every enabled,
  runnable startup check succeeded. The generation comes from the default
  check's lookup (systemd `InvocationID`, container id), which now always
  runs, also when default checks are off.
- **Not run yet.** Exec and shell checks, and headers with a vault
  reference, report `unknown` with `unsupported_executor`. They need the
  confined probe worker and per-probe vault access (still to do).
- **Policy push without a new command kind.** The policy travels as the
  existing signed app command with the new action `health` and a
  `forge_catalog::health::ApplySpec` (namespace, name, runtime, listening
  port, policy, revision: no environment). The agent handles it itself,
  validates the policy again, and accepts it only for an app that is
  deployed on the node. This replaces the planned `ApplyHealth` command
  (proto tag 35) and the `app_health_applied` table: the command kind `app`
  is already in every agent's local policy, so no node needs
  `offer-commands`, and no migration is needed.
- **Reconciliation by revision.** `revision(listen_port, policy)` is a
  content hash (FNV-1a over the JSON form; `0` for "defaults only", which
  is also what a node reports after a deploy). Every report carries the
  revision it ran under. `PUT /applications/{id}/health` pushes at once; a
  heartbeat that reports another revision than the stored policy's
  triggers a push, at most once per node, application and revision every
  10 minutes. So a node that was offline, or was redeployed, converges
  without a per-node "applied" record.
- **Status.** `GET /applications/{id}/health` lists configured checks a
  node hasn't reported yet as `unknown` (`received_at: null`) and leaves
  out results of checks the policy no longer has.

**Why.** A stored "applied revision per node" duplicates what the node
already says in every report, and can disagree with it. The report is the
evidence.

**Consequences.** An agent older than this refuses the `health` action
(the helper doesn't know it) and keeps running default checks only; Forge
retries every 10 minutes until the agent is updated. A probe to a remote
host is not checked against the fleet's addresses on the node, only when
the policy is saved. The push throttle is in the server's memory: after a
restart each node may get its policy once more.

## D-106 Buckets attached to applications: a tagged vault item, injected at deploy

**Context.** Object stores, buckets and access keys existed on their own.
Giving an application a bucket meant issuing a key, revealing its secret and
pasting endpoint, bucket and both key halves into the application's
environment by hand. The owner asked for object storage to be part of
applications.

**Decision.**
- **An attachment is a store key with two more tags.**
  `POST /applications/{id}/storage` issues a bucket-limited key (the same
  RustFS service account as `POST /object-stores/{id}/keys`) and tags its
  vault item `app:<application id>` and `env-prefix:<PREFIX>`, next to the
  existing `objstore:`, `bucket:` and `access:` tags. No new table and no
  migration; `GET` and `DELETE` read the same items.
- **Injected at deploy, never copied.** `app_storage::deploy_env` runs in
  the `app.action` job after settings are expanded: for each attachment it
  adds `<PREFIX>_ENDPOINT`, `_REGION`, `_BUCKET`, `_ACCESS_KEY_ID` and
  `_SECRET_ACCESS_KEY` to the environment sent to the node. The
  application's stored environment is not touched, so the secret has one
  home (the vault), and rotation and detach need no second write.
- **The application's own variables win.** Attaching is refused (`409`)
  when the prefix is already used by another attachment or when the
  application defines one of the five variables; at deploy an existing
  variable is kept.
- **Permissions.** `deploy` on the application's namespace and
  `storage.manage` (MFA) on the store, both, for attach and detach;
  `view` on the namespace to list. Audited on the application as
  `application.storage.attach` and `.detach`, with the access key but never
  the secret.
- **Two steps, compensated.** The key is created at the store, then recorded
  in the vault. When recording fails, the key is deleted again.
- **A missing store fails the deploy.** The node is not sent an application
  whose attached store no longer exists. Detaching then removes the record
  only.

**Alternatives.** Writing the five variables into the application's
environment (the secret sealed like other secret variables) was rejected:
two copies of the secret, and rotate, detach and store changes would each
need a second write that can fail half way. A separate attachments table was
rejected for now: the vault item already is the record of the key, and the
tags are how Forge recognises its keys.

**Consequences.** Changes take effect at the next deploy; until then a
detached or rotated key leaves the running application with credentials
that no longer work. Listing attachments scans the workspace's vault items.
Only managed RustFS stores can be attached. Next slices, asked for by the
owner: object stores that belong to an application and environment, and
access for people per bucket.

*Amended by D-107: a store can belong to an environment, and attaching a
bucket of a store to a service of the owning environment needs `deploy`
only. Access for people per bucket is still open.*

## D-107 An object store can belong to an environment

**Context.** Every object store was a platform resource: a managed store was
authorized on its member node, a connected store on the workspace. A team
with a role on one environment could not see or browse "its" store without a
role on the node, and attaching a bucket needed `storage.manage` (MFA) even
for a store installed for that one environment. The owner asked for stores
that belong to an application and environment (D-106, next slices).

**Decision.**
- **One nullable owner.** `ObjectStore` gets `namespace_id` (migration
  `0005_object_store_owner.sql`, a nullable column with an index). `NULL` is
  a shared store. A store has at most one owner, and the owner is an
  environment (namespace), never an app group.
- **Unowned stores are unchanged.** Every existing store stays shared, and
  every rule for a shared store is what it was.
- **The owner has its own write path.** `put_object_store` writes
  `namespace_id` on insert only. Afterwards only
  `set_object_store_owner` changes it. A job that saves a copy of the store
  it loaded earlier therefore cannot undo a change of owner.
- **Authorization is an added path, at the API.** The node or workspace
  check stays as it is and is never replaced. For a store with an owner, the
  API also accepts `view` and `storage.data` when the caller holds that
  action on the owning namespace (`owner_grants` in `api/storage.rs`). The
  list endpoint asks for the owning namespaces in one batch. Attaching or
  detaching a bucket for a service of the owning namespace needs `deploy` on
  that namespace only; a shared store or a store of another environment
  still needs `storage.manage` (MFA) on the store as well.
- **The boundary is the store, not the bucket.** The roles of the owning
  environment reach every bucket of the store. An environment should own a
  store only when all its buckets belong to that environment; a store used
  by several environments stays shared, with `storage.manage`. Two checks
  keep ownership from opening another environment's bucket unnoticed:
  assigning a store while its buckets are attached to applications of other
  namespaces answers `409 attached_elsewhere` unless the request has
  `force: true`, and the audit event then lists those attachments; and on
  the `deploy` shortcut a bucket that an application of another namespace
  has attached falls back to the `storage.manage` check
  (`409 bucket_attached_elsewhere` when it fails). An attachment whose
  application no longer exists counts as another namespace's.
- **Detach needs `deploy` only,** on the service's namespace, whoever owns
  the store: it revokes the service's own key and grants nothing.
- **The owner path sees less.** A caller whose access comes only from the
  owning namespace gets a typed reduced store (`limited: true`: id, name,
  slug, provider, mode, topology, status, endpoint, owner, member hostnames
  and status) without the credential id, data paths, ports, capacity, last
  error or jobs, and of the store's keys only those attached to
  applications of that namespace. The node or workspace rule is asked
  first, so operators always get the full view.
- **Denials name the store.** A refused request on an owned store is audited
  on the node or workspace with `store_id` and `namespace_id` in the
  details.
- **Everything under `storage.manage` stays on the node or workspace:**
  install, repair, lifecycle, uninstall, disconnect, bucket deletion,
  standalone keys, and changing the owner.
- **Schema.** `storage.data` `appliesTo` gains `Namespace`. No new action, no
  new role grant, and the MFA guardrails are untouched.
- **Setting the owner.** `namespace_id` on install, connect and adopt, and
  `PUT /object-stores/{id}/owner` afterwards (`null` clears). Assigning needs
  `storage.manage` (MFA) on the store and `deploy` on the target namespace,
  which must be in the caller's workspace (`404` otherwise). Clearing needs
  `storage.manage` only. Audited as `storage.owner.set` with `from` and `to`.
- **No dangling owner.** `DELETE /namespaces/{id}` answers `409` while the
  namespace owns a store. The check is part of the store adapter's delete
  transaction, and `set_object_store_owner` writes the owner only when the
  namespace exists, in one statement; it also sets `updated_at` and returns
  `NotFound` when nothing was updated. On SQLite the two cannot interleave.
  On PostgreSQL (read committed, no foreign key) a concurrent pair can
  still do so in theory.

**Alternatives.** Making the store a Cedar entity with the namespace as its
parent was rejected: it changes the resolver and the schema for every store
call, and `storage.manage` would then be inherited by the namespace's
operators, which is root on the node. A new action such as `storage.use` was
rejected: it grows the role matrix without adding a capability that `view`,
`storage.data` and `deploy` do not already describe. Owning by an app group
directly was rejected: a binding on an app group already covers its
environments, so it covers their stores.

**Consequences.** Attaching inside the owning environment needs no MFA
step-up: a developer can issue a bucket-limited key for their own
environment's store, and can read, change and delete the objects of every
bucket in it. Every environment role with `view` sees the store's endpoint
and the names of its member nodes. A role on the store's member node (or on
the workspace for a connected store) reaches the store as before, so an
environment's roles are not the only people who reach its store. The operators of the store's
node keep full control whoever owns it. Changing or clearing the owner moves
no data and leaves existing keys and attachments working. Next slice, still
open: access for people per bucket.

*Followed by D-108: bucket grants give one person access to one bucket.*

## D-108 Bucket grants: one person, one bucket, through Forge

**Context.** Access to an object store was all or nothing. A role on the
store's node or workspace, or on the owning environment (D-107), reaches
every bucket of the store. The owner asked for access per person and per
bucket: an administrator lets one person work with one bucket, read-only or
read-write, optionally for a limited time, without giving that person a role
on the store, its node, the workspace or its environment. Tunnel grants
(D-100) are the precedent: an explicit, revocable record next to the role
model, for one person and one target.

**Decision.**
- **A grant is a row, not a role.** Table `bucket_grants` (migration
  `0006_bucket_grants.sql`): workspace, user, store, bucket name, access,
  issuer, created, expires, revoked, note. "Never" is the same sentinel
  `expires_at` as in `tunnel_grants`. `revoked_at` is the only column that
  changes after a grant is issued. The port is
  `forge_core::storage::BucketGrantRepository`; the rules live in the
  use-case type `BucketGrants` in `forge-server`, which takes ports only
  (grants, stores, users, `Clock`). Nothing changes in the Cedar schema or
  the role matrix.
- **Access is through Forge only. No S3 key is issued.** A grant opens the
  console, the REST API and the CLI. The holder gets no credential and no
  endpoint; Forge makes the S3 calls with the store's own credential, as it
  does for every caller. S3 clients and applications keep using access keys
  and attachments (D-106).
- **Two levels.** `read` lists and downloads objects; `readwrite` also
  uploads and deletes objects. The default is `read`.
- **Enforced in three places only.** The four object endpoints
  (`GET …/buckets/{bucket}/objects`, `GET|PUT|DELETE …/buckets/{bucket}/object`)
  ask the role rules first (node or workspace, then the owning namespace)
  and only then look for a live grant of the caller for that store and that
  exact bucket name with enough access (`authorized_for_bucket`).
  `GET /object-stores` and `GET /object-stores/{id}` show a grant-only
  caller the reduced store with `by_grant: true`. `GET …/buckets` shows that
  caller only the granted buckets. Every other storage endpoint needs a
  role as before: bucket creation and deletion, keys, the connection test,
  lifecycle, attachments, the owner and the grants themselves.
- **Checked on every request.** A grant counts while it is not revoked, not
  expired by the `Clock` and its holder's account is not disabled. It is
  read for each request, so a revocation or an expiry holds from the next
  one. There is no cache and nothing to close.
- **The reduced view hides more for a grant.** The owner path (D-107) shows
  the endpoint, the owning namespace and the member host names. The grant
  path leaves all three out (`endpoint` empty, `namespace_id` null,
  `members` empty): the holder works through Forge alone and has no use for
  them.
- **Managing needs `storage.manage` with MFA, listing included.** Creating,
  revoking and listing a store's grants are authorized on the store's node
  or workspace, never on the owning namespace and never through a grant, so
  a grant cannot be passed on. Reading the list is treated like reading
  everyone's tunnel grants. `GET /object-stores/grants/mine` gives every
  account its own live grants. Using a grant needs no MFA, like
  `storage.data`.
- **No expiry by default.** A grant lasts until it is revoked unless the
  request gives `lifetime_minutes` (1 minute to 10 years) or `expires_at`
  (a future moment); both together are refused.
- **One live grant per person and bucket.** A second one answers `409`;
  changing the access or the expiry means revoking and issuing again. The
  check and the write are not one transaction: two simultaneous requests
  can leave two grants, each listed and revocable on its own, and neither
  gives more than was asked for.
- **Validated when issued.** The bucket must exist at the store (`404`), so
  a typo does not leave a grant that opens whatever bucket gets that name
  later. The person must be an existing account that is not disabled; both
  failures answer the same `404 unknown or disabled user`, so the endpoint
  does not say which accounts exist. A request that is allowed but fails is
  audited (`failure`) with the store, the bucket and the login as given,
  and the error's code, not its text. Users are per deployment (D-016).
- **Grants end with their bucket.** A grant names its bucket by name, so
  it must not reach another bucket that later gets the name. Deleting a
  bucket through Forge revokes its live grants; creating a bucket through
  Forge (the bucket endpoint, or an attach that creates it) revokes any
  grant still live for that name, which also covers a bucket deleted outside
  Forge. Each is audited as `storage.grant.revoke` with the reason
  `bucket deleted` or `bucket recreated`. The store call and the revocations
  are not one transaction: if revoking fails after a delete, the request
  fails and the creation path revokes what is left.
- **A revocation is final.** The adapter's upsert keeps an existing
  `revoked_at` (`COALESCE`), so no later save moves it or clears it.
- **The holder's own list is reduced.** `grants/mine` answers id, store,
  bucket, access, expiry and status: not the note and not who issued it.
- **A holder is not shown the store's errors.** On the grant path every
  store failure answers `502 store_unreachable` with a fixed text (`404` and
  `400` likewise fixed); the address and the store's own message go to the
  server log. Callers a role let in keep the full message.
- **Grants go with the store.** `delete_object_store` deletes the store's
  grants in the same transaction (uninstall that removes the store,
  disconnect).
- **API keys and service accounts, not AI connectors.** A grant counts for
  the holder's sessions, API keys and service-account tokens, as tunnel
  grants count for API keys. It never counts for an AI connector
  (`grants_apply`): not on the connector's own MCP connection, and not when
  a person approved the connector's request and it is replayed with that
  person's session (the `ApprovalExec` marker, D-073). A connector gets what
  its user's roles allow and nothing an administrator handed to the person.
- **Audit.** `storage.grant.create` and `storage.grant.revoke` on the
  store's node or workspace. `storage.object.download`, `.upload` and
  `.delete` carry `grant_id` (null when a role allowed the request). A
  refused object request is a denied `storage.data` with the store, the
  bucket and the access it needed.
- **One definition of the access level.** The `Access` enum of the key and
  attachment code moved to `forge_core::storage::BucketAccess`, used by
  keys, attachments and grants alike.
- **CLI.** Flat names next to `keys` and `key-create`:
  `forge storage grants`, `grant-create`, `grant-revoke` and `my-grants`.
  `--for` uses the tunnel grant duration syntax and defaults to `never`.

**Alternatives.** Issuing a personal S3 key per grant was rejected: only
managed RustFS stores can issue keys, so connected stores would be left out;
the secret would have to reach a person; a revocation would depend on the
store instead of taking effect with the next request; and S3 traffic with
that key bypasses Forge's audit log. A Cedar role bound to a bucket entity
was rejected: buckets are not entities in the database, they live in the
engine, so there is nothing to bind to and nothing to keep in sync. Reusing
the `tunnel_grants` table was rejected: its target model (service, node,
ports) does not describe a store and a bucket, and its lifetime rules are
tunnel settings. For the reduced view, showing the endpoint as on the owner
path was considered and rejected: a grant holder cannot use it.

**Consequences.** Every request on the grant path reads the holder's grants,
one indexed query (`bucket_grants_user`), plus the account for the disabled
check, and a list names people by reading only the accounts it mentions.
Bucket names are matched exactly. A bucket that is deleted and created again
outside Forge keeps a grant that is still live; administrators revoke grants
when they delete a bucket that way. A grant is checked when a request
starts, so a transfer already running finishes after a revocation. Listing
objects through a grant is not audited, as it is not for roles; transfers
and deletes are. There are no grants per prefix, no grants to groups and no
personal S3 key for a holder. The list of a store's grants returns the
newest 200 by default (at most 500). A list of stores by environment
(`?namespace_id=`) leaves out stores reached only through a grant, so a
holder cannot learn the owner the reduced view hides.

## D-109 The CLI drives a deployment end to end; a container deploy installs Docker Engine

**Context.** Setting up the lab with the CLI alone (issue #64) needed
`forge api request` for applications, service definitions, placement roles,
attached buckets, a store's owner, single objects and the ingress profile.
`forge apps deploy` said it installs missing runtimes, but a container on a
node without Docker failed with the node's "Docker is not available": only
the runtimes a definition lists in `dependencies` were installed, and Docker
Engine is not a runtime package.

**Decision.**
- **Names follow the console.** `forge applications` is the console's
  *Application* (API `app-groups`); `forge apps` stays the service (API
  `applications`), and `forge namespaces` the environment. The API keeps its
  names (D-102).
- **Flags and files share one body.** `forge apps create | update` and
  `forge clusters ingress set` build the REST body from `--file`, from flags,
  or from both (flags change the file's fields). An update without a file
  starts from the stored definition, so a flag changes one field and keeps
  the rest, stored secrets included. Bodies built from flags pass the same
  schema check as files (D-077).
- **Secrets never come from argv.** `--secret-env KEY` reads stdin,
  `KEY=@FILE` a file; `KEY=value` is refused.
- **Objects go through Forge.** `forge storage object-upload | object-download
  | object-delete` use the store's object endpoint, so roles, bucket grants
  and the audit log apply (D-108). Uploads stream from disk with a length.
- **A container deploy installs Docker Engine on a node that has none**, in
  `POST /applications/{id}/deploy`, with the App Store's install job
  (`package.manage`, MFA), before the deploy jobs. A node gets the install
  only when Forge has no Docker Engine installation for it (installed or in
  progress) *and* the node itself answers that Docker is not available. The
  server asks with a read-only `docker info` command, only a node whose
  agent polled this process recently, and waits at most 15 seconds. The
  deploy job waits for a running install and fails with a clear message when
  the install it waited for failed.
- **One definition of the node's error.** The agent's result is text, so the
  control plane recognises the answer by
  `forge_catalog::apps::DOCKER_UNAVAILABLE`, which the agent also uses.
- **An answer without a state keeps the last one.** A container's `follow`
  segment carries no state; it used to clear the instance's state, so
  `forge apps list` showed `0/N` running after the console or the CLI
  followed logs.

**Alternatives.** Installing Docker whenever Forge has no record of it was
rejected: the recipe would run over an engine installed by hand and restart
its containers. Starting the install from inside the deploy job, after the
node refused, was rejected for now: the job would need to submit and
authorize another job, and the caller would not see the install in the
answer. Making Docker a `dependencies` entry was rejected: every container
definition would have to list it. A typed error code in the agent's result
was rejected: it changes the wire protocol for one case.

**Consequences.** The deploy request can take up to 15 seconds per node that
is connected but does not answer the probe. A node that is offline at deploy
time gets no install; its job fails with the node's error and the next
deploy installs. Deploying one instance through the per-node action installs
nothing. An agent older than the constant still answers with the same text.
`SiteSpec.upstreams` defaults to an empty list, so a site body with only
`targets` is accepted; validation still requires one upstream after
resolution.

## D-110 Alarm notifications: channels, policies and an outbox with durable delivery (issue #11, slice 3)

**Context.** D-087 left notifications to a later slice and fixed one rule
for it: evaluation never sends anything. Issue #11 asks that an alarm
transition and its notification intents cannot be lost between two writes,
that a failed message can never rerun a remediation or erase an alarm, and
that a webhook cannot be used to reach what only the control plane reaches.
The dependency tree has no SMTP client and no Handlebars renderer.

**Decision.**
- **Vocabulary.** `forge_core::notifications`: a *channel* (a transport), a
  *policy* (which alarms notify which channels), a *delivery* (one event of
  one alarm for one channel of one policy, with its attempts) and one port,
  `NotificationRepository`. The service is `forge_alerts::notify::Notifier`;
  it takes ports only (`NotificationRepository`, `AlertRepository`, `Clock`)
  and three small traits the server implements: `Transport`,
  `ChannelSecrets`, `ResourceNames`.
- **Channel kinds.** `webhook` posts Forge's JSON payload and can sign it;
  `slack` posts `{"text": …}` to Slack incoming webhooks and compatible
  receivers. Both need no new dependency (`reqwest`, `ring`, `hex` and `url`
  are already in the tree). Email is not in this slice.
- **Outbox in the alarm's transaction.** `open_alarm` and `close_alarm`
  insert a row into `alarm_transitions` (`alarm_id`, `kind`: one of `fired`,
  `resolved`, `ended`) in the transaction that changes the alarm. The
  primary key is (alarm, kind): an alarm fires once and closes once. There
  is no sequence number and no cursor, so the `MAX(seq) + 1` race of D-085
  does not arise; the planner reads rows where `planned_at IS NULL`.
- **Planning is one transaction per transition.** `Notifier::plan` matches
  the workspace's policies and calls `plan_transition`, which marks the
  transition planned and inserts its deliveries together. Each delivery has
  a unique `dedupe_key` (alarm, event, occurrence, policy, channel), so a
  replay by the planner or behind its back adds nothing. A channel reached
  through two matching policies gets one message per event.
- **Dispatch is a lease, not a job.** `Notifier::dispatch` takes a due
  delivery with one conditional `UPDATE` (`pending` and due, or `sending`
  with an expired lease), which also counts the attempt; the attempt row and
  the delivery's new state are written together. The durable job engine was
  not reused: a job is authorized as a person, audited per step and visible
  under **Jobs**, and its handlers can act on nodes. A notification has no
  acting person and must not be able to act on anything. Delivery is at
  least once: an accepted message whose answer is lost is sent again with
  the same delivery id (`X-Forge-Delivery`).
- **Retries.** 2xx is `sent`. `408`, `425`, `429`, `5xx`, timeouts,
  connection and TLS failures retry with `forge_core::Backoff` (half to all
  of the ceiling, spread by the delivery id) up to `[alerts]
  delivery_attempts` (default 6), then `failed`. Any other status, a
  blocked target, a missing signing secret and an unusable stored URL fail
  at once. An attempt whose worker died still counts.
- **Messages that no longer apply are cancelled, with the reason.** A
  `fired` or `reminder` delivery is dropped when its alarm stopped firing; a
  reminder also when the alarm was acknowledged. A `resolved` or `ended`
  delivery goes only where a firing message for that alarm arrived (or
  where the policy listens for `resolved` only); it waits while that
  message is still in flight.
- **Reminders.** A policy may set `repeat_interval_secs` (5 minutes to 7
  days). The reminder's number is the count of whole intervals since the
  alarm fired and is part of the dedupe key, so a planner that ran late
  adds one reminder, not a backlog. At most 50 per alarm and channel.
- **`ended` is not recovery.** An alarm that ends without recovery evidence
  notifies under the policy's `resolved` event, as event `ended` with
  `is_resolved: false` and the reason.
- **Fixed payloads, versioned.** One JSON document (`schema_version` 1) for
  webhooks and one fixed text for Slack-compatible receivers, both built
  from typed values and serialized, never assembled as strings. Text values
  are made inert (markup escaped, `@` mentions broken, one line, 200
  characters). A payload holds alarm facts and one console link.
- **Signature.** `X-Forge-Signature: v1=<hex HMAC-SHA256>` over
  `<X-Forge-Timestamp>.<body>` with `ring`. The key is the password of a
  credential vault item named by the channel (`secret_item`), read when the
  message is sent and never copied into the channel.
- **The URL is a secret.** Webhook URLs often carry a token, so the URL is
  sealed with the envelope (AAD bound to the channel id), never returned,
  logged or audited; answers show scheme, host and port. Failures are
  recorded as a class (`timeout`, `connect`, `tls`, `blocked_target`,
  `http_status`, `secret`, `config`), never as the HTTP client's text,
  which can quote the URL.
- **Target rules, applied twice.** `notify::target` builds on
  `forge_catalog::health::{forbidden_address, canonical, numeric_host,
  forbidden_name}`. HTTPS only; link-local, metadata, multicast and reserved
  addresses never; loopback and private ranges only with `[alerts]
  webhook_allow_loopback` and `webhook_allow_private`; `http://` only with
  `webhook_allow_http`. The rules run on the URL when a channel is saved
  and on every address the name resolves to at each connection; a name with
  any refused address is refused. The transport follows no redirect, uses
  no proxy, verifies TLS, allows 5 s to connect and 15 s in all, and never
  reads the answer's body. The object-store guard in `api/storage.rs` keeps
  its own, older rule set; moving it onto `notify::target` is a follow-up.
- **Authorization.** New action `notification.manage` (platform and
  workspace administrators, under the MFA guardrail) for creating,
  changing, deleting and testing channels: a channel is an outbound
  destination, so it is stricter than `alert.manage`, which covers
  policies. Reading channels, policies and deliveries is `alert.view`.
  Naming a vault item needs the right to use it (platform administrator, a
  session, `secret.read`), checked again when the item or the URL changes.
  Deliveries of alarms on resources the caller cannot `view` are not
  listed. Channel and policy changes and test sends are audited; sending is
  not (the delivery history is its record).
- **A test is an explicit, audited action.** `POST
  /notification-channels/{id}/test` sends one message in the request, with
  one attempt, and answers with the result.

**Alternatives.** Computing the deliveries inside the evaluator and passing
them to `open_alarm` was rejected: evaluation would then read notification
configuration and fail with it. A sequence cursor like D-085's feed was
rejected: only this consumer exists, and a `planned_at` mark has no
ordering race. Storing the webhook URL as a vault item was rejected for
now: the vault is for platform administrators only, which would have kept
workspace administrators from creating any channel. Adding `lettre` and
`handlebars` in this slice was rejected: each is a dependency to audit, and
neither is needed for a first complete delivery path.

**Consequences.** Migration `0007_notifications` adds five tables. Final
deliveries are deleted after 30 days. The signing secret of a channel can
only be set by a platform administrator at a browser. A receiver sees a
repeat when its answer was lost; the delivery id lets it drop one. A proxy
for outbound notifications is not supported. Every channel of a workspace
is visible to everyone with `alert.view`, and every policy can use every
channel.

**What remains of #11.** Email (SMTP) and provider channels; Handlebars
templates with the editor and preview; silences and maintenance windows;
grouping, digests and escalation steps; quiet hours; quotas and
`Retry-After`; delivery receipts; acknowledgement links; alarms on node,
service and container pages; notification tools for MCP; the remaining
rule sources (job and backup outcomes, certificate expiry).

**Amendment (security review of D-110).**
- **A transition that cannot be planned never blocks the queue.** Each
  transition is planned on its own; a failure is counted on its row
  (`plan_failures`, `plan_error`) and the pass goes on. After 5 failed
  passes the transition is marked planned with no deliveries and the reason
  kept. Reminders are planned even when the transitions could not be read.
- **Each channel is its own lane.** The store returns at most two due
  deliveries per channel, longest due first (a window over `channel_id`),
  so a backlog on one channel cannot fill a batch. A lane sends one
  delivery at a time and stops for the pass at its first timeout; two
  timeouts in a row pause the channel for 60 s. The pause is a field of the
  `Notifier`, in memory: forgetting it on restart costs one timeout.
- **IPv6 transition prefixes are refused whole**: 6to4, Teredo, local-use
  NAT64 and IPv4-translated (`forge_catalog::health::transition_address`).
  Health-check targets do not use the rule yet; that is a follow-up for
  #12, since it changes what an existing policy may probe.
- **A late attempt changes nothing.** `finish_attempt` is fenced on the
  attempt number and reports whether it still held the delivery.
- **Tests are rate-limited and always audited.** One test per channel per
  10 s (`429`); accepted, failed and refused tests are all audited.
- **Lookups by id do not reveal existence.** A channel, policy or delivery
  the caller may not use answers the same `404` as an unknown id; the
  denial is still audited. `step_up_required` passes, because only a caller
  whose role allows the action receives it.
- **Policy audit** records rules and scope, before and after an update.
- **Text for chat** also loses invisible and direction-changing characters
  and never contains `://` from a name or a value.
- **Signing secrets are at least 16 bytes**, checked when a channel is
  saved and again at every send. Receivers are told to trust the signed
  body only, not the `X-Forge-*` headers.

## D-111 Command health checks run in the root helper by reference; vault header values are fetched per check; target rules hold at probe time (issue #12, slice 3b-3)

**Context.** D-105 left three things of #12 open. Exec and shell checks
were saved but reported `unsupported_executor`: D-085 allows them only as
the application's own account, confined like the file worker, or inside the
application's container, and the unprivileged agent can do neither.
Headers with a `vault:<id>` reference were not sent: the value must never
be in the policy on the node. And a probe to a remote host was checked
against the fleet only when the policy was saved.

**Decision.**

- **The helper runs command checks, and only what Forge signed.** The agent
  cannot switch accounts or reach Docker; the root helper can, and already
  verifies signed commands (D-068). The signed app command `health` (D-105)
  now also goes to the helper, which keeps the enabled exec and shell
  checks of the app in its root-only folder
  (`/var/lib/forge-agent-helper/health/<ns>--<app>.json`). A signed deploy
  stores the policy it carries at revision 0, as the agent reports it, and
  a removal drops it. The agent then asks with the new request
  `health_check` and a reference only: namespace, app, purpose, check name
  and policy revision. It is the one unsigned request that runs code
  (`allowed_unsigned`), and it is safe to allow because the reference
  carries no command, arguments or script (`deny_unknown_fields`, slugs
  only), the helper looks the check up in what it stored, and another
  revision than the stored one is refused. Code running as the agent's user
  can therefore start a check Forge authorized, at the pace the helper
  allows, and nothing else. An unsigned `health_policy` is refused by a
  pinned helper like every other privileged request.
- **A stored policy has an age and an order.** The helper keeps the time
  Forge issued the command (`issued_at_ms` of the signed body, filled in by
  `Trust::admit`, never by the agent) and refuses a policy issued before
  the one it holds, so a delayed or replayed command cannot bring back
  checks a newer policy removed. A policy without command checks is stored
  too, for that memory. The helper runs nothing from a policy it has not
  received for 30 minutes (`PolicyExpired`); the server sends the policy of
  an application with command checks or a remote target again every 10
  minutes while the node reports, so three sendings must be missed.
- **A policy that adds or removes command checks is applied only when the
  helper took it.** Otherwise the agent fails the command and keeps its
  previous policy, so the agent and the helper never run different
  revisions for long, and the helper never keeps checks the agent's policy
  no longer has; Forge sends the policy again.
- **Native apps: a worker as the app's account, in a cgroup, under
  Landlock.** The helper resolves the account on the node
  (`apps::app_account`, never from the request), creates a cgroup v2 for
  the run (`/sys/fs/cgroup/forge-health/<ns>--<app>--<purpose>--<check>`,
  `pids.max` 64, `memory.max` 256 MiB, no swap), starts its own binary as
  `health-worker` with the app's uid and gid, no other group and a cleared
  environment, moves it into the cgroup, and only then hands it the
  command. The worker checks every id of its process (as the file worker
  does), lowers its limits, confines itself and starts the command. The
  command gets `PATH`, `HOME`, `USER`, `LOGNAME` and `LANG`, the app's
  folder as its working directory, and none of the app's environment. An
  exec probe is its argv; a shell probe is its interpreter with the script
  on stdin, so no shell of ours reads the text and it is not in the node's
  process list.
- **Nothing outlives a run.** When the run ends or its timeout passes, the
  helper writes `cgroup.kill` and removes the cgroup; dropping the request
  (the agent went away) does the same. A process cannot leave its cgroup
  without write access to another one, which the account does not have, so
  `setsid`, a new process group or a double fork change nothing, and a fork
  bomb stops at 64 processes. The first version killed the worker's process
  group, which a command leaves with one `setsid`; the review caught it.
- **What Landlock does and does not do here.** Every filesystem right
  through ABI 3 is a hard requirement of the ruleset: read and execute
  beneath the system folders and the app's folder, read three devices,
  write only to `/dev/null`. A check can therefore not create, write,
  append to, truncate, rename or delete a file anywhere. ABI 3 (Linux 6.2)
  is the least accepted, because below it truncation is not controlled; the
  worker reports the kernel's ABI and the helper refuses below 3. Device
  ioctls (ABI 5) and the scoping of signals and abstract UNIX sockets (ABI
  6) are added where the kernel has them. Landlock covers none of these at
  any ABI: changing the mode and timestamps of files the account owns,
  connecting to UNIX sockets by path, and the network, which is left open
  on purpose because most checks ask the app over TCP. The run also sets
  `no_new_privs`. It is confinement of writes, not a read-only sandbox.
- **No confinement, no run.** The file worker confines itself best effort
  (D-083), because the operation it runs is Forge's own code. A command
  check is the operator's code, so it does not run without Landlock ABI 3
  (`NotConfined`) or without a cgroup v2 with the pids and memory
  controllers (`NotContained`), and the helper does not believe a worker
  that says it ran unconfined. A PID namespace as the fallback for nodes
  without cgroup v2 was not built: it needs `unshare`, which the workspace's
  ban on `unsafe` rules out, and every kernel with Landlock ABI 3 has
  cgroup v2 and `cgroup.kill`. Each refusal has its own sentence
  (`health_exec::Refusal`), and none of them is a failure. The switch that
  lets the worker run unconfined exists only in builds with the cargo
  feature `test-unconfined`, used by the root test on a kernel without
  Landlock.
- **Containers: Engine API exec in the app's own container.**
  `forge-app-<ns>-<app>`, and only when it carries Forge's label for the
  app (`forge.app = <ns>/<app>`): the name alone cannot tell `a-b`/`c` from
  `a`/`b-c`, and anyone with Docker access can name a container. The
  container's default user, no TTY, no stdin, nothing privileged, no added
  environment. A shell probe is `<interpreter> -c <script>` here: exec
  gives a command no stdin without a hijacked connection, so the script is
  in the process list of the container and of the node. Closing the stream
  does not end an exec'd process, so a guard ends it when the timeout
  passes or the request is dropped: it asks Docker for the pid, pins it
  with a pidfd, checks in `/proc/<pid>/cgroup` that it belongs to that
  container and signals it through the pidfd; a recycled pid is never hit.
  A run that cannot be ended is remembered and the check is not started
  again until it is gone (`timeout`).
- **Output is an opt-in.** A command can print a secret, and a check's
  detail is stored and shown. By default a failed run reports its exit code
  only. The new policy field `show_output` (exec and shell, default off)
  reports the last 160 printable bytes of a failed run; it needs
  `health.command` like every command check. A pass reports nothing either
  way.
- **Limits.** One check never runs twice at once and its starts are 0.9 s
  apart whoever asks; 4 command checks per node and 2 per application run
  at once, besides the agent's 8; a check waits up to 2 s for a place. Three
  periods in a row without a place count as a failed attempt (`timeout`),
  so a check that never gets to run turns failing by its own threshold
  instead of staying neutral.
- **`unknown` is a result of its own, and says whether waiting helps.**
  `health_probe::Outcome` has a verdict (pass, fail, unknown) and, for
  unknown, a kind. Unknown counts toward no threshold. Only a permanent
  refusal (not Linux, no Landlock ABI 3, no cgroup v2, no account, a
  foreign container) releases the startup gate; a refusal that passes on
  its own (a policy on its way or expired, a busy node, a helper or Forge
  not answering, a refused vault value) leaves the startup check `waiting`.
- **Vault header values: asked for per check, kept in memory.** New agent
  request `POST /agent/v1/health-secrets` (mTLS, protobuf
  `HealthSecretsRequest`/`Response`): the node names one of its checks and
  the revision it runs. `HealthSecrets::resolve` reads the references from
  the stored policy, never from the request, and decides every time: the
  node is within 120 requests a minute; the app is placed on the asking
  node; the node runs the stored revision; somebody saved exactly that
  revision; that person still exists, is enabled, is a platform
  administrator and passes `secret.read` on the workspace; every item
  still exists in that workspace; and the audit entry that is due was
  written. The Cedar context says `mfa: true`: a probe has no session, the
  save that named the person passed the MFA guardrail, and what is checked
  again is what they may do now (as the fabric reconciler and automatic
  agent updates do with `AppliedBy`). A grant carries the values and a
  lifetime of 300 s; the agent keeps them in memory for that long at most
  (600 s whatever the server says), per check and revision, and drops them
  when the time is up or the check is disabled or removed. A refusal
  carries a code and is kept for 60 s. Refused or unanswered, the check is
  `unknown` (`permission_denied`, `invalid_config` for a missing item,
  `location_unavailable` when Forge does not answer) and nothing is sent to
  its target. The value is the item's password, as the whole header value.
- **The authority is the saver's, for the revision they saved.** Table
  `app_health_authority` (migration 0008): application, person, time and
  policy revision. `PUT …/health` writes it and the policy in one
  transaction (`save_health_policy`); a failed save changes nothing and is
  audited as failed. A policy whose revision changed another way (the
  listening port is part of the revision; a write that bypassed the
  endpoint) has no authority until someone who may use the items saves it
  again.
- **A vault value never travels in clear text or to an unverified peer.**
  `HealthPolicy::validate` refuses a header with `secret` unless the probe
  is HTTPS with certificate verification or targets the workload itself.
  The agent checks again on the connection it made: without verified TLS
  the peer must be a loopback address, whatever the host was called.
- **Audit at the rate of change, and before release.**
  `application.health.secret_use`, actor `agent:<node>`, with the check,
  the revision, the item ids and `on_behalf_of`: the first answer for a
  check on a node, every change of the answer, and one entry a day while
  nothing changes. The entry is written before the value is returned; if
  it cannot be written the answer is a refusal, and the answer is
  remembered as audited only once the entry exists. Requests that name no
  real check on the node are refused without an entry, so a node cannot
  fill the log; they are counted and logged at most once a minute. What was
  audited and the request counts are fields of `HealthSecrets`; a restart
  writes each entry once more.
- **Target rules at probe time.** `transition_address` (D-110) is now part
  of `HealthPolicy::validate` and of the agent's `allowed_targets`: 6to4,
  Teredo, local-use NAT64 and IPv4-translated addresses are refused whole.
  A policy that probes a remote host is sent with the addresses of the
  other nodes of the application's workspace (`ApplySpec::fleet`, at most
  2048, a cut is logged: where each agent connects from, the addresses in
  its facts and its fabric address; the receiving node's own addresses are
  left out, since nodes behind one NAT share one). The agent refuses a
  target that resolves to one of them, in every spelling `canonical`
  knows. The list is not part of the revision; it travels with the
  10-minute re-send. If it cannot be built, nothing is sent and the push
  is not counted as done.

**Alternatives.**

- *The agent passes the command to the helper* (argv or script in the
  request, perhaps with a hash): rejected. It makes "run this as an app's
  account" a request the agent's user can forge; by reference, the helper
  needs no trust in the agent at all.
- *A per-probe signed command from the control plane*: rejected. A check
  every 10 s per app and node would put the fleet's probing on the control
  plane and stop when it is unreachable.
- *Kill the worker's process group on timeout*: built first, rejected in
  review: `setsid` leaves the group.
- *Run the command in the app's systemd unit (`systemd-run`, `nsenter`)*:
  rejected for now. It ties checks to systemd, and joining the unit's
  namespaces as root is a larger surface than a worker that starts as the
  account. The cgroup gives the containment without it.
- *Run unconfined where Landlock is missing, as the file worker does*:
  rejected by D-085's invariant; the operator gets a clear `unknown`.
- *Always report the end of a failed run's output*: built first, rejected
  in review. The detail is stored and shown to everyone with `health.view`.
- *Send the secret with the policy, sealed for the node*: rejected. It
  would be at rest on the node, and revoking the saver's right would
  change nothing.
- *The helper fetches the secret*: rejected. The helper has no link to
  Forge, and the HTTP probe runs in the agent.
- *Audit every fetch*: rejected. With a 5-minute lifetime that is 288
  entries a day per check and node, which hides the entries that matter.

**Consequences.**

- Wire: one new agent path and three protobuf messages; `ApplySpec` gains
  the optional `fleet`; exec and shell probes gain the optional
  `show_output`; the helper protocol gains `health_policy` (signed, with
  the time of issue) and `health_check` (by reference). No new command
  kind, so no node needs `offer-commands`. Migration 0008 adds one table.
- Old agent, new server: as before, command checks and vault headers
  report `unsupported_executor`; `fleet` is ignored. A policy that sets
  `show_output` is refused by an older agent (unknown field) until the
  node is updated. New agent, old server: the request for a vault value
  gets no answer and the check is `unknown` (`location_unavailable`);
  everything else works. Agent and helper are updated together
  (self-update); a helper that does not answer `health_check`, because it
  is not running or is older than its agent, gives one sentence that says
  both.
- Command checks need Linux 6.2 or later (Landlock ABI 3) and the unified
  cgroup hierarchy with the pids and memory controllers on native apps.
  Ubuntu 24.04 (6.8) qualifies. The stock kernels of Debian 12 (6.1,
  Landlock ABI 2) and Ubuntu 22.04 (5.15, ABI 1) do not: native command
  checks report `unknown` there until the node runs a newer kernel (for
  Debian 12, the backports kernel). That is the price of refusing to run
  with truncation uncontrolled. Container command checks need neither,
  and HTTP and TCP checks run everywhere.
- A node that loses Forge for 30 minutes stops running command checks
  (they turn `unknown`) until the policy arrives again; HTTP and TCP checks
  go on, and cached vault values end after 5 minutes.
- A revoked role, a disabled saver or a deleted item takes effect within
  the lifetime of the last grant (5 minutes). The value is in the agent's
  memory for that time: code running as the agent's user on that node can
  read it, and can also ask for it, for the checks of the apps placed on
  that node only.
- Changing an application's listening port changes the policy revision:
  checks with vault headers turn `unknown` until the health policy is
  saved again by someone who may use the items.
- A command check may read what the app's account may read (its folder,
  its env file, world-readable files), change the mode and timestamps of
  the app's own files, and reach the network. It cannot write, cannot gain
  privileges, cannot outlive its run, and cannot exceed its process and
  memory limits on a native app. In a container, children of a timed-out
  command may survive: Docker ends only the process it started.
- A native command that exits but leaves a process holding its output
  open has not finished: it runs into the timeout.
- The helper's stored checks follow signed commands only. They are not
  removed when an app's account is deleted by hand; such a check is refused
  (`NoAccount`).
- The helper's state for command checks (who runs, the app limits, stuck
  container runs) is a `health_exec::Runner` owned by its serve loop.
- Tested as root in WSL (kernel 6.18, Landlock ABI 6, cgroup v2 with
  `cgroup.kill`): identity, groups, environment, working directory, argv,
  script on stdin, `no_new_privs`, create/write/append/truncate/remove
  refused while `chmod` and `/dev/null` work, daemons in new sessions
  killed after a passing run and after a timeout, a fork bomb held under 64
  processes, overlap, pacing, expiry, every refusal, and removal of the
  cgroups. Not exercised: a kernel below ABI 3 or without cgroup v2 (the
  refusals are unit-tested), the memory limit, and Docker: exec is covered
  by unit tests of the request it builds, the label and cgroup checks, the
  stream reader and the guard only.

**What remains of #12.** Verification on the lab nodes and in a browser
(native and container command checks against a real Docker engine, a vault
header against a real target). An age limit on the saver's authority, if
wanted (automatic agent rollouts use 90 days). Killing the children of a
timed-out command inside a container. Passing a container shell script on
stdin. Command checks on Windows.

## D-112 Silences and maintenance windows hold back notifications, never evaluation (issue #11, slice 4)

**Context.** Operators need to stop being paged during planned work or
while they investigate, without losing the record of what happened. D-087
and D-110 fixed two rules: evaluation never sends anything, and sending
never writes to an alarm. A silence has to fit both, and an operator of one
environment must not be able to quiet the whole workspace.

**Decision.**
- **One object.** `forge_core::silences::Silence`: a scope, optional rules,
  optional `min_severity` and `max_severity`, `starts_at`, an exclusive
  `ends_at` and a required reason. A maintenance window is a silence whose
  start is in the future; its state (`scheduled`, `active`, `expired`) is
  derived from the clock, never stored. Limits: 1 minute to 30 days long,
  up to 365 days ahead, 50 silences that have not ended per person and
  200 per workspace (people with `alert.manage` on the workspace are
  exempt from the workspace cap, so narrow roles cannot use it up), 100
  rules and 100 scope ids.
- **Fixed after creation.** Only the end moves: extend (later end, at most
  30 days after the start) or end now. What a silence matches never
  changes, so the audit record of its creation stays true. An expired
  silence never changes.
- **Checked in one place.** `Notifier::gate` consults the active silences
  just before a send. The evaluator never reads them; alarms, their
  timelines, acknowledgements and notes are unchanged. A covered delivery
  becomes `suppressed` (a new final status) with `silence_id` and the
  reason in `detail`. Matching is against the alarm (workspace, scope,
  rule, severity), not an alarm id, so a recurrence inside the window is
  covered.
- **One message after.** When the silence has ended, the planner settles
  what it held back per alarm and channel, in one transaction
  (`release_suppressed`): still firing gives one `fired` (or one
  `reminder` when only reminders were held); closed after the channel was
  told it fired gives one recovery; fired and closed inside the silence
  gives nothing. That message has `after_silence` set and the payload
  carries `after_silence.silence_id`; its dedupe key names the silence, so
  a replay adds nothing. While another active silence still covers the
  alarm, the held deliveries move under it.
- **Severity ceiling.** An alarm worse than `max_severity` is not
  silenced, so a maintenance window can keep `critical` paging.
- **Authorized on the scope.** `alert.view` and `alert.manage` now also
  apply to a cluster, a namespace, a server and an application. Creating,
  extending and ending need `alert.manage` on everything the scope names;
  reading needs `alert.view` on the workspace or on everything the scope
  names. A scope covers only what sits under what it names, the tree its
  authorization is checked on: a node silence does not cover applications
  running on that node, which may belong to an environment the person
  holds no role on. A workspace manager can always extend or end a
  silence, also one that names a deleted node. An unreadable silence
  answers `404` (a refused change is still audited); an unknown scope entity is
  refused like one without a role, so ids cannot be probed. Every change
  is audited (`alert.silence.create`, `alert.silence.extend`,
  `alert.silence.end`) with what it matches and the window, and the
  previous window for changes.

**Alternatives.** Muting in the evaluator (skip opening alarms) would lose
history and break "missing data is never recovery". Muting at planning time
would miss deliveries planned before the silence and retried during it.
Replaying every held message after the silence would flood receivers with
stale states. Editable silences were rejected to keep the audit trail
honest. Recurring windows were left out of this slice.

**Consequences.** Migration `0009_silences` adds the `silences` table and
`silence_id`, `after_silence` and `released_at` to
`notification_deliveries`. A recovery whose firing message was suppressed
is cancelled ("the firing notification was silenced, so there is nothing to
recover from"). Ended silences are pruned with the delivery retention once
no delivery names them. Test messages name no alarm and are never
silenced. Not available: recurring windows, editing what a silence covers,
silencing by label.

## D-113 Email notifications via lettre; allow 0BSD for quoted_printable only (issue #11)

**Context.** Many teams want alarms in a mailbox, not only in a chat or a
webhook. Sending email means speaking SMTP with STARTTLS or implicit TLS,
authentication, MIME encoding of non-ASCII subjects and long lines, and
precise reply-code handling. Writing that by hand is a source of security
bugs; an SMTP library is the smaller risk.

**Decision.**
- **Library.** `lettre` 0.11 (MIT), default features off; `builder`,
  `smtp-transport`, `pool`, `tokio1`, `tokio1-rustls`, `ring` and
  `rustls-native-certs`. TLS is rustls on `ring` with the operating
  system's roots: no OpenSSL, one crypto provider. It is a dependency of
  `forge-server` only; the agent and `forge-core` are unaffected. lettre
  pulls `quoted_printable` (0BSD, public-domain equivalent): `deny.toml`
  allows 0BSD for that crate alone. It also brings `nom` 8 next to the
  `nom` 7 that `x509-parser` uses; accepted until `x509-parser` moves.
  `hostname` is not enabled: the `EHLO` name is set explicitly from the
  public URL's host (`ClientId::Domain`), or `localhost`.
- **Layering.** `forge-alerts` owns the `MailTransport` port
  (`mailbox()` to check an address, `send()` of an `OutboundMail`) and
  builds the subject and body from the same `Payload` as the other kinds.
  `forge-server::notify_mail::SmtpMailer` is the only lettre user, wired in
  `state.rs` next to the HTTP transport.
- **Settings and secret.** Kind `email` with an `email` object: host, port
  (default from the TLS mode: 587, 465, 25), `tls` (`starttls` default,
  `tls`, `none`), optional user name, sender, 1–20 recipients, optional
  subject prefix. The non-secret settings are stored as JSON in a new
  `notification_channels.email` column (migration `0010_email_channels`)
  and returned, with `password_set`. The password is sealed in the
  existing `sealed_url` column with the channel-bound envelope, exactly like
  a webhook URL: write-only, never returned, never logged or audited.
  Leaving it out of an update keeps it while the user name stays; removing
  the user name drops it. A sealed password is never reused as a URL when
  the kind changes.
- **Target rules.** The SMTP host passes the webhook rules when saved and
  when sent; the mailer resolves the host itself, refuses it when any
  address is forbidden, and connects to a checked address while TLS still
  verifies the host name. STARTTLS is required, never opportunistic.
  `tls = none` needs `[alerts] webhook_allow_http` and is refused with a
  user name: a password is never sent in the clear.
- **Message.** Plain text only. Subject
  `<prefix> [FIRING|REMINDER|RESOLVED|ENDED|TEST][<severity>] <title> — <resource>`
  plus ` (after silence)`; body with the Slack text's facts, UTC times and
  the console link. Values are stripped of control and invisible characters
  and cut to 200 characters, so a name cannot add a header or a line.
  `Message-ID` is `<delivery-id@public-host>` (stable across retries) and
  `Auto-Submitted: auto-generated` prevents auto-replies. `Date` comes from
  the `Clock` port.
- **Retries.** A new outcome `SendOutcome::Smtp(code)` and failure class
  `smtp_reply`: 2xx sent; 4xx retried on the normal schedule; 5xx failed at
  once. The reply code is kept in the existing status field
  (`last_http_status`, `http_status`); connect, timeout and TLS failures
  retry as for webhooks. A server that does not offer STARTTLS is `tls`.
  The server's reply text is never stored: it can quote the user name.
- **Permissions and audit** are those of every channel:
  `notification.manage` with MFA; audit details carry the `email` settings
  and `password_changed`, never the password.

**Alternatives.** A hand-written SMTP client (no new dependency) was
rejected for the TLS upgrade and MIME encoding it would have to get right.
Storing the password in a vault item, like a webhook signing secret, would
have made every email channel need a platform administrator to set up; the
password belongs to the channel as a webhook URL does. HTML email was left
out: plain text carries the facts and cannot render injected markup.
Recording the reply in a separate column was rejected for reusing the
status field, which keeps the delivery tables unchanged.

**Consequences.** One more migration (`0010_email_channels`). The CLI gains
`--smtp-*`, `--tls`, `--from`, `--to` and `--subject-prefix` on
`forge alerts channels create|update`; the password is read with
`--smtp-password @FILE|@-`, never from the command line. Not available:
HTML bodies, attachments, per-policy recipients, OAuth2 SMTP login and
client certificates.

## D-114 Container shells run through the agent link and the root helper

**Context.** Container shells (D-101) went over SSH as the node's saved
terminal account with `sudo docker exec`. A node without a terminal account,
or one without passwordless `sudo`, could not open one, and the shell needed
a credential on the host for a task inside a container.

**Decision.**
- A signed `ContainerExecOpen` (proto tag **39**; 35 stays reserved, 36 and
  37 are skipped) asks the agent for a shell. Its `name()` is
  `container_exec`, its own allow-list entry: new enrollments allow it, but
  `offer-commands` never adds it to an existing `allowed_commands`, so
  existing agents opt in by adding it (`container` does not admit it). The
  agent checks it (ids, token, expiry, limits), forwards the command as Forge
  signed it to the root helper on its existing Unix socket, and the helper
  verifies it with its own pin (replays refused by operation id). The helper
  request `ContainerExec` is never parsed from a request line, and a helper
  without a pin refuses it even in signed form ("container shells need a
  pinned helper"). The agent's fact `container_exec` is true only when the
  helper answers the read-only `capabilities` request with a pin, so Forge
  does not offer shells on such nodes.
- **The helper enforces limits itself.** The signed `idle_secs`,
  `lifetime_secs` and `expires_at_ms` travel into the helper's `ExecSpec`;
  it refuses an expired command and ends a shell after the idle time
  (Docker traffic either way) or the lifetime, whatever the agent does.
- **Scope.** `container.exec` on a node allows a shell in any container on
  that node, including ones Forge did not create, as the SSH path did. It
  is not limited to Forge-managed containers.
- The helper drives the Docker Engine API: exec create (`Tty`, stdin, stdout
  and stderr attached, `TERM=xterm-256color`,
  `sh -c 'command -v bash … && exec bash || exec sh'`), start hijacked
  (`Upgrade: tcp`), resize, and the exit code from exec inspect at the end.
  It answers one JSON line, then the socket carries
  `forge_proto::exec_frame` frames: `[kind u8][len u32][payload]` with
  Data, Resize, Eof, Exit, Error and Ready, payload at most 64 KiB.
- The agent dials `/agent/v1/exec/<session>` with a single-use token in
  `x-forge-exec-token`, like tunnels (D-091), sends `Ready` (or `Error` with
  the helper's reason) and relays the helper's frames unread. The server's
  `container_exec.rs` registry stores only the token's hash, accepts the
  stream once and only from the signed node, and adapts it to
  `ShellReader`/`ShellWriter`, so the terminal relay, the WebSocket frames,
  the 30 s access re-check, idle (30 min) and lifetime (12 h) limits and
  revocation through remote sessions are those of SSH terminals. `closed`
  adds `exit_code`.
- **Path choice.** Without `login=` the shell takes the agent path. `login=`
  is an explicit opt-in to the SSH path, which is otherwise unchanged. There
  is no automatic fallback to SSH.
- **User.** The shell runs as the image's default user. There is no user
  parameter and no `container.exec_as` action.
- **Capability check.** The server gates on the node fact `container_exec`
  (reported by agents that support it, Linux only) instead of comparing
  agent versions: builds share one version number, and the same pattern
  serves node-bound certificates (D-069). Missing → `409
  agent_update_required`. As a backstop, a node that neither dials back nor
  reports within 15 s gets "the agent did not answer: update the agent".
- **Limits.** Eight shells per node (the helper is authoritative; the agent
  and server check early) and eight per user. The server reserves the place
  in its remote-session registry when it checks, before the WebSocket
  upgrade, releases it once the session is registered and checks again, so
  concurrent opens cannot pass a limit. The agent's and the helper's counters
  belong to the agent and to one run of `serve` (no process-wide static).
  Bounded queues (8 × 64 KiB) at every hop plus the HTTP/2 window give
  backpressure.
- Audit: `container.exec` and `container.exec_close` with `path: agent`,
  duration, bytes, reason, exit code and node-side error. Refusals before the
  shell opens (Windows, offline, missing fact, limits, CSRF) are audited as
  `container.exec` with outcome denied and the reason.

**Consequences.** No credentials or sudo rule are needed for a container
shell. It needs a current agent and a Linux node; Windows nodes get `409`.
Ending a shell closes its PTY, which hangs up the shell, but Docker cannot
kill an exec, so a process that ignores SIGHUP keeps running. Podman is out
of scope.

**Rejected.** Passing the hijacked socket to the agent with SCM_RIGHTS (a new
dependency or `unsafe`, and the helper is still needed for resize); giving
the agent the Docker socket (root-equivalent); creating a terminal account
automatically (a credential and sudo on the host for a container task);
version-number gating (see above).

## D-115 Postgres runs a patched copy of migration 1, with the original checksum

**Context.** `0001_initial.sql` declares `servers.cluster_id REFERENCES
clusters(id)` before `clusters` is created. SQLite accepts a forward
reference; Postgres rejects it, so `forge-server init` failed on every
Postgres database. Nothing tested the `postgres` feature, so this went
unnoticed. Migration 1 is in every released build, so editing the file would
change its checksum and break every existing SQLite install on startup.

**Decision.** The file stays as released. On Postgres, `Store::connect` runs
a copy of the migrator in which migration 1 declares the column without the
constraint and adds `servers_cluster_id_fkey` with `ALTER TABLE` at the end.
The copy keeps the original checksum, so both backends record the same
history. The patch is an exact string match and fails loudly if the text
ever differs. A unit test checks the order and the checksums; a second test
applies all migrations to the Postgres database in
`FORGE_TEST_POSTGRES_URL` when that variable is set.

**Consequences.** The resulting Postgres schema is the intended one. New
migrations must not reference a table before it exists (Postgres checks;
SQLite does not). `fleet-sim` measures Postgres when `FORGE_DATABASE_URL` is
set.

**Rejected.** Editing `0001` and accepting the old checksum (rewrites
released history); a separate Postgres migrations folder (two sets to keep in
sync forever).

## D-116 Local-names sync: debounced passes, shared lists, batched reads (amends D-084)

**Context.** fleet-sim at 1,000 agents showed the names sync dominating:
every heartbeat from a node without a list poked a full rebuild, so passes
ran back to back while a fleet enrolled or reconnected. Each pass loaded the
facts, instances and settings once per workspace, fetched applications one by
one, scanned the server list per instance, and built and hashed a separate
list for every node (a million entries at 1,000 nodes). Every send re-read
the settings for the suffix. Reconnect storm: 47 s (SQLite), 67 s (Postgres)
against 11–13 s with names off.

**Decision.**
- A poke waits `DEBOUNCE` (2 s); pokes during the wait or the pass fold into
  one more pass.
- A pass reads servers, instances, applications (`get_applications`, one
  query per 200 ids) and the suffix once for the whole fleet; facts only when
  some node has no live address. Lookups are hash maps, not scans.
- Lists are built once per (fabric, loopback owner) and shared as `Arc`s,
  with one hash each. A unit test proves them equal to `entries_for` per node.
- A changed list goes to a node at most once per `CHANGE_GAP` (10 s); first
  sends, reconnects and the 10-minute repair are unchanged.
- Observation writes (same benchmark): `latest_for` reads up to 50 keys per
  statement, and `apply` reads `MAX(seq)` once per batch under a per-store
  async lock, which also stops two Postgres transactions from taking the same
  sequence number.

**Consequences.** During a burst a node may hold a list up to 10 s older
than the newest one. Per-node lists, the API and the agent protocol are
unchanged.

**Amendment (same change): fleet-sim no longer touches the host.** Simulated
agents run in the fleet-sim process and executed the names command for
real: on a Windows host where `Users` may modify the hosts file, every
agent rewrote it (with fsync) under the agent's process-wide `HOSTS_LOCK`,
one after another. That alone added about 30 s to the reconnect storm at
1,000 agents and left a `sim-*` block in the host's hosts file. fleet-sim
agents now allow only `ping` and `collect_facts` and refuse everything else
locally; the server-side cost is still measured. Separately, the agent
retries opening a hosts file only on a Windows sharing or lock violation;
access denied fails at once instead of sleeping 1.6 s under the lock.

## D-117 Policy recipients: email people by user and role, resolved per alarm (issue #11)

**Context.** An email channel sends to a fixed list of up to 20 addresses.
Teams want alarms to reach the people responsible for the resource that
fired, and those people change with role bindings. Copying addresses into
channels goes stale and leaks addresses to people who manage channels.

**Decision.**
- **Model.** A notification policy gets `email_channel` (an `email`
  channel of the workspace), `email_users` (up to 100 user ids, each with a
  role in the workspace) and `email_roles` (up to 20 role names). The
  channel is required when either list is set. `channels` may then be
  empty. Stored as one nullable JSON column `notification_policies.email_recipients`.
- **Resolution at plan time, in `forge-alerts`.** A new `People` value
  holds the ports it reads (`UserRepository`, `RoleBindingRepository`,
  `Authorizer`, `ResourceResolver`); `state.rs` wires the store and the
  Cedar authorizer. Named users and the holders of a role on the alarm's
  resource or any ancestor (`ResourceRef::is_within`, so workspace and
  platform bindings count and siblings do not) are unioned; everyone must
  pass `alert.view` on the resource; disabled accounts and accounts without
  an address are skipped; addresses are deduplicated case-insensitively and
  capped at 200. Role holders pass the same `alert.view` check as named
  users, so a role that cannot see alerts never receives them.
- **One delivery per alarm event, policy and email channel**, with its own
  dedupe key (`…|people`), so it never collides with the same channel
  attached as a channel. The resolved addresses and the skipped people (id
  and reason, never an address) are snapshotted in a nullable
  `notification_deliveries.recipients` JSON column. Retries and the
  message after a silence reuse the snapshot. Silences, reminders,
  recoveries and after-silence messages work per lane: a channel's own
  recipients, or one policy's people. A set that resolves empty is stored
  as `cancelled` with `no recipients`.
- **Mail.** People are sent blind copies (`OutboundMail::bcc`): envelope
  recipients without a header, so nobody sees the other addresses. The
  channel's static `to` is not added.
- **Privacy.** API answers show delivery addresses only to callers with
  `notification.manage`; others see `count` and the skipped ids. Policy
  audit details hold the ids and roles. Policy answers carry
  `warnings.users_without_email` for the editor.
- **Migration** `0011_policy_recipients` adds the two nullable columns.

**Alternatives.** Resolving recipients when the policy is saved was
rejected: it would freeze role changes and keep addresses in the policy.
Re-resolving on every retry was rejected: a retry should reach the same
people as the first attempt. Sending one email per person was rejected for
now: one SMTP transaction per delivery keeps the retry and dedupe model of
D-110; blind copies give the same privacy. A separate "user has no access"
pass only for named users was rejected in favour of the same check for
everyone.

**Consequences.** Planning reads users and bindings for every policy that
emails people; reminders skip the resolution when their delivery already
exists. A user without an address is skipped silently except for the
warning and the delivery detail. Not available: per-person preferences,
on-call schedules, account lockout as a skip reason (only `disabled`
exists), and recipients beyond 200 per delivery.

## D-118 Alert subjects for nodes, services, databases and certificates; rule templates (issue #11)

**Context.** Rules could watch `node.agent.status`, CPU, memory, the root
disk and health checks, on nodes and applications only. The console's
subject list was hard-coded, a typo in a subject saved a rule that never
fired, and operators had to build every common rule by hand. They asked
for alerts on full disks, reboots, failed instances and deployments,
stopped databases and expiring certificates, offered where those things
are shown.

**Decision.**
- **New subjects from the heartbeat** (`measures.rs`, no agent change: the
  telemetry already carries them): `node.disk.max_used_percent` (the
  fullest local volume; removable media and virtual filesystems excluded;
  the mount point in the observation detail), `node.swap.used_percent` (0
  without swap), `node.load.per_cpu` (`load1 / cpu_count`, new unit
  `ratio`, unknown without a CPU count) and `node.uptime.seconds`.
- **One series per node for disks, not per mount.** An observation key is
  resource, node and subject, and subject segments cannot hold a path. A
  per-mount subject would need a mount entity or an encoded subject; the
  worst volume with its mount in the detail answers "is any disk full" and
  keeps one alarm per node. `node.volume.used_percent` is not provided.
- **Control-plane subjects** computed by `ControlPlaneSource` on each pass
  from the registry, valid for two passes: `node.cert.expires_in_days`,
  `app.instance.state` and `app.deploy.status` (resource the application,
  `server` its node), `package.state` and `package.status` (per
  installation, databases included) and `tls.cert.expires_in_days` (per
  uploaded ingress certificate). Recording them as observations was
  rejected: the registry is already the source of truth, and a second copy
  would need its own freshness and clean-up.
- **Installations and certificates as alarm resources.** `EntityKind`
  gains `installation` and `certificate`; the resolver chains them to their
  node or workspace. They are not Cedar entities: the alarm list checks
  `view` on an installation's node (the alarm's `server`) and on a
  certificate's workspace, as the package and certificate lists do.
  `RuleScope` gains `installations {ids}` and `certificates {ids}`; ids are
  checked against the workspace on save.
- **Catalogue in `forge_alerts::catalog`**, the one definition: every
  subject with value kind, states or unit, scope kinds and source;
  `check_subject` refuses unknown subjects and subjects of another scope
  kind on save (`400`, with "did you mean" within an edit distance of 3).
  `health.<readiness|liveness|startup>.<check>` is a pattern. An unknown
  `node.*` subject is still accepted on a node scope, so a newer agent's
  measurement can be watched before the server knows it.
- **Templates** are typed (`AlertTemplate` with a `forge_rules::RuleSpec`),
  listed by place (node, service, database, certificate), and may use a
  `{check}` placeholder. A unit test parses and validates every template
  and checks its subject and states against the catalogue.
- **API** `GET /alert-subjects` (with up to 200 `health.*` subjects seen on
  the workspace's applications) and `GET /alert-rule-templates`. **CLI**
  `forge alerts subjects`, `forge alerts templates` and `create-rule
  --template` with typed target flags. **Console** a rule dialog fed by
  both endpoints (scope kinds, repeated mode, `=`/`≠`), and **Add alert**
  on the node page, the Health Checks tab (per check: **Alert when this
  fails**), package rows and uploaded certificates.

**Not done.** A database port check (`package.port.reachable`): health
probes are planned and reported per application instance; probing a
package's own port needs a probe target that is not an application, so it
is left for a later change. Alarm messages name an installation or a
certificate by id (the names port covers nodes and applications).

**Alternatives.** Templates in the console only were rejected: the CLI and
API would drift. Making installations Cedar entities was rejected for now:
no action is authorized on them, and visibility through the node is what
the package list already does.

## D-119 Alert emails carry a designed HTML part next to the plain text (issue #11)

**Decision.** An email notification is `multipart/alternative`: the
existing plain-text body first (unchanged), then `multipart/related` with an
HTML rendering and the brand mark as an inline PNG
(`Content-ID: <forge-mark@orbyn-forge>`). `forge_alerts::notify::email::html`
builds the HTML from the same `Payload` as every other channel, plus the
policy and channel names of the delivery; `OutboundMail.html` carries it and
the SMTP transport builds the MIME tree with lettre's `builder` feature.

**Why.** Operators read alarm mail on phones and in Outlook; a coloured
state banner, a sentence in plain words, "what to check" hints and one
button to the alarm get someone acting faster than a field list. The plain
text stays for filters, text-only clients and screen readers.

**How it is kept safe and portable.**
- Table layout, inline styles only, 600 px wide, system fonts; no style
  sheet, script, web font, remote image or tracking pixel. `color-scheme:
  light` and explicit colours on every cell, so a dark-mode client does not
  produce unreadable text.
- Every dynamic value goes through `mail_plain` (one line, bounded,
  nothing invisible, `://` defused) and then one `escape` helper. The only
  `href`s are the console URL from the payload, and only when it is an
  absolute `http(s)` URL (no `public_url`, no buttons).
- The explanation and the checks are chosen by observation-subject family
  (`node.agent.status`, `node.cpu|mem|disk`, other `node.*`, `health.*`,
  `app.*`, `package.*`, `tls.*`, anything else) and by event (`resolved`,
  `ended`). They are fixed text in code until notification templates exist.
- The mark is `branding/forge-mark.svg` rendered once to a 128 px PNG
  (`src/apps/forge-server/assets/email/forge-mark-128.png`, about 5 KB,
  made with `sharp` from the docs toolchain) and compiled into the server
  with `include_bytes!`. Mail clients block SVG and remote images. No new
  dependency.

**Not now.** Environment and service rows (the payload does not carry
them yet), a node name for application alarms (the id is shown), a
per-channel choice of text-only mail, a silence form prefilled from a
link (the console opens silences by id only, so the email links to the
alarm, whose drawer has **Silence**), and dark-mode-specific colours.

## D-120 Runtime public address; Forge published on a domain through an ingress; Settings and Profile

**Context.** Forge is reached directly by IP (`public_url`, for example
`http://192.0.2.1:55130`). The owner also wants it on a domain through the
cluster ingress edge (Nginx) that already serves other portals: the edge
terminates TLS and proxies plain HTTP to the server. Links in alert emails,
webhooks, approvals and device sign-in were built from `public_url`, and the
WebSocket and `/mcp` Origin checks compared against it only.

**Decision.**
- A runtime **public address** (`server.public_address` in the settings
  store) overrides `public_url` for every link people follow: alert email
  and webhook/Slack `links.alarm`, MCP approval links, device sign-in
  `verification_uri`, the OpenAPI `servers` entry and the OIDC redirect URL
  (read at start). Empty or absent: `public_url`. One shared value
  (`forge_core::public_address::PublicAddress`, a field on `Services` and in
  `NotifyConfig`), read per message, so console and API changes apply at
  once.
- The value is an **origin only** (`http(s)://host[:port]`): no path, query,
  fragment or user info, at most 255 characters, stored canonical. Forge is
  served at the root, so a path prefix would only produce broken links.
- Changing it needs `system.update` on the platform and an MFA-verified
  session, like the local-name suffix; reading needs `view` on the platform.
  Audited as `server.public_address.update` / `.clear`, refusals as denied.
  The generic settings editor refuses `server.` keys.
- **Both ways in keep working.** Allowed browser origins are exactly
  {origin of `public_url`, origin of the public address}. The `Host` header is
  never trusted.
- **Cookies:** `Secure` when `cookie_secure = true`, or when the TCP peer is in
  the existing `[security] trusted_proxies` and its last `X-Forwarded-Proto`
  is `https`. From any other peer the header is ignored. No new config key:
  `trusted_proxies` already existed for `X-Forwarded-For`.
- **Ingress:** no code change. Sites already take a raw `host:port` upstream
  or a typed `external` target, already pass WebSockets and set
  `X-Forwarded-Proto`, and already issue Let's Encrypt or use uploaded
  certificates. The guide uses that. No `forge` target kind was added.
- **Change paths:** console **Administration → Settings** (a new
  server-level page), `forge settings public-address get|set|clear` (API),
  `forge-server public-address get|set|clear` (writes the database on the
  host; restart to apply to a running server), and `install.sh
  --public-address <url> --trusted-proxies <list>`.
- **Console naming:** the per-user page that was **Settings** is now
  **Profile** (`/profile/<section>`; `/settings/<section>` and the old anchors
  redirect). **Settings** (`/settings`) is the server-level page; future
  server settings go there.

**Not now.** Passkeys stay bound to the host of `public_url` (WebAuthn
relying party); the MCP OAuth issuer/resource and the SFTP host default stay
on `public_url` (identifiers, not links); a server-side refresh of the cached
value after `forge-server public-address` (restart instead); a nav gate that
hides **Settings** from non-platform roles (the page explains the 403);
`install.ps1` options for the public address.


## D-121 Application Explorer: a view of `/app-groups`, joined in the browser (issue #74)

**Decision.** The Application → Environment → Service tree is a third view
(**Explorer**) of the **Applications** page, next to List and Cards, rather
than a new page. Its state (`view`, `sel`, `q`) lives in the URL. The tree is
joined in the browser from the existing list endpoints (app groups,
namespaces, applications with instances, servers); no tree endpoint was added.
Two small server additions keep it free of per-row requests:
`GET /applications/health-summary` (per-node check status of up to 500
applications, filtered by `health.view`) and `can_deploy` on application
views (`deploy` on the namespace, checked with `can_many`), so the console can
disable actions it knows are refused. The server still authorizes each action.

Running state (health checks, falling back to the node-reported unit state)
and rollout state (`deploying`, `deploy failed`) are shown as two separate
signals. A service appears once per environment; placements are tags on its
row. Logs are opened per placement from the detail pane, so the node is never
implicit; the "All nodes" view merges up to eight follow loops in the browser
by timestamp. No streaming log endpoint was added.

**Not done.** List endpoints are still unbounded, and the UI has no component
test framework yet; adding vitest is a separate decision.

## D-122 Direct port tunnels over the node's SSH (issue #35, phase 2)
**Decision.** `forge tunnel open --direct` connects to the node's SSH
endpoint instead of relaying through the control plane. The tunnel is still
opened and held through the server (`POST /tunnels` with `direct: true`, the
control WebSocket). For each accepted connection, the CLI asks for a
certificate (`POST /tunnels/{id}/ssh-cert`). The server re-runs the 15 s
access check and returns:
- the endpoint and its pinned host key;
- a fresh Ed25519 key;
- a certificate valid for 60 s, with these properties:
  - its only principal is `forge.<server_id>.forge-tunnel` (node-bound, D-069);
  - its only extension is `permit-port-forwarding`;
  - its key id is `forge-tunnel/<tunnel>/<address>/<port>`.

The node's sshd now passes the key id to the principals command
(`ssh-principals %u %i`). For the `forge-tunnel` account, the helper answers
`restrict,port-forwarding,permitopen="<address>:<port>"`. It refuses an
unparseable key id, a link-local, unspecified, multicast or broadcast
address, and port 0. `port-forwarding` alone would also allow Unix-socket
forwarding, remote listeners and an `internal-sftp` subsystem, so the helper
ends Forge's drop-in with a `Match User forge-tunnel` block:
`AllowTcpForwarding local`, `AllowStreamLocalForwarding no`,
`PermitListen none`, no tty, X11 or agent forwarding, and
`ForceCommand nologin`. The agent reports `ssh_tunnel` only with that block
in the drop-in. The helper creates `forge-tunnel` as a system account
with no home, `nologin` as its shell and password field `*`. The agent
reports `ssh_tunnel` in its facts. The CLI connects with russh (already in
the tree, ring) and accepts only the pinned host key. The key stays in
memory.

**Why.** The relay doubles the path and puts all traffic on the control
plane. Many operators can reach their nodes' SSH directly. Taking the
destination from the CA-signed key id lets one sshd configuration line limit
each certificate to one destination without per-tunnel files on the node.

**Consequences.**
- Only *new* direct connections are cut at once when a tunnel is closed,
  expires or loses access: a certificate is valid for a minute and each
  connection needs a fresh one.
- The CLI drops open connections when the control socket closes. This is
  cooperative, not enforced, which is documented.
- The server records no byte counts for direct tunnels. The CLI sends
  `{"type":"active"}` on the control socket so the idle timeout still holds.
- Existing nodes switch at their next helper start (agent update). The old
  `%u` line is rewritten in place.
- Stored facts can predate that switch, so a direct open re-collects facts
  once before it refuses, at most once a minute per node.
- Windows nodes are not supported.

## D-123 User groups, held-over grant checks and service-level authorization (issue #13, phase 2)
**Decision.** Access profiles (D-086) can be assigned to **groups**. A user
can be in many groups, a profile can be assigned to many groups and users,
and grants are a union; there is no deny. In Cedar, `entity Group` is new
and `User in [Group]`; the role and profile templates use
`principal in ?principal`, so one template linked to a group grants every
member. Memberships are loaded into the engine at start and changed there
together with the store: added after the store write, removed before it, so
removal ends access at once and a failed write fails closed.

Group assignments live in a **separate table**, `group_assignments`
(migration `0012_access_groups.sql`, with `access_groups` and
`access_group_members`). The released `profile_assignments` table keeps its
shape (one user per row). The domain type `ProfileAssignment` carries
`user_id` or `group_id`, exactly one set.

Group CRUD needs `access.manage` on the workspace; membership needs
`user.manage` on the workspace. Adding a member is handing out everything
the group holds, so it needs MFA and passes the escalation guard at every
scope the group is assigned. Removing a member or deleting a group needs no
MFA.

The escalation guard and `GET /access/effective` judge an action as **held
over** a scope: the request is evaluated against the same policies and
entities but not validated against the schema, so a node-only action such
as `container.manage` granted at an environment or cluster counts there.
This check is only for deciding what may be granted and for display; real
requests are still authorized on the exact resource.

Services are authorized on the service itself (`Application`, whose parents
are its environment and the workspace): the list, detail and actions. The
schema now lets `view`, `deploy` and `job.create` apply to `Application`,
`user.manage` to `AppGroup`, `Server` and `Application`, and `Job` have an
`Application` parent. `app_group` is a new assignable scope. A deploy with
privileged mounts stays on the environment, because `stack.privileged` is
held per environment.

**Why.** Assigning the same profile to each person does not scale and
drifts. A separate table avoids rewriting a released migration's table and
keeps old rows valid. Without held-over checks an administrator could not
grant node actions at the level they actually manage. Authorizing on the
service makes "restart one service" expressible.

**Consequences.**
- Restarting one service needs `view`, `deploy` and `job.create` on it.
- Groups are local to Forge; no identity-provider group sync.
- Group operations are audited under the platform as `group:<id>`, since
  groups are not Cedar resources.

## D-124 Restricted personal access tokens (issue #13)
**Decision.** A personal API key can carry a **ceiling**:
`restrictions: [{scope: "<kind>:<id>", actions: [..]}]`, 1 to 50 entries
(`MAX_TOKEN_GRANTS`), on assignable scopes that exist. It is stored as JSON
in `api_keys.restrictions` (migration `0013_token_restrictions.sql`);
`NULL` keeps the earlier behaviour, all of the owner's access. At every
request the token may act only when the owner is allowed **and** an entry
lists the action at a scope covering the resource (`ceiling_allows`); list
filtering (`can`, `can_many`) and the escalation guard (`holds_over`) apply
the same intersection. At creation the creator must hold every listed action
over each scope (`403` `escalation`). The CLI takes repeatable
`--allow <kind>:<id>=<action>[,…]` and resolves `cluster:<name>`; the
console offers **All my access | Only selected**.

**Why.** A CI key for one environment should not carry an administrator's
whole reach. A ceiling rather than a grant keeps one source of truth for
access: the key can never exceed its owner, and losing a role narrows the
key at once.

**Consequences.**
- Jobs are checked against the ceiling at submit time only; later steps
  re-authorize the owner (the job principal), not the ceiling, for the same
  action and resource.
- Service-account tokens are not restricted this way; their own
  assignments narrow them.
- API keys still cannot create API keys.

## D-125 Docker Engine API over mutual TLS

**Decision.** A node's dockerd can listen on TCP (default `2376`) with
mutual TLS, and the control plane forwards the full Docker Engine API to it
at `/api/v1/servers/{id}/docker/engine/{path}`. Enabling is a
`docker.endpoint.enable` job (`prepare`, `sign`, `apply`, `verify`):
- the root helper makes dockerd's key in `/etc/docker/forge-tls` (it never
  leaves the node) and sends a CSR;
- a **per-node Docker CA** signs dockerd's certificate (DNS name
  `<server_id>.docker.forge.internal`, the listen and dial addresses as IP
  SANs, 825 days) and the control plane's client certificates (24 h, in
  memory, renewed at two thirds). Its key is sealed with the master key,
  associated data `docker-ca:<server_id>`, in `docker_endpoints`
  (migration `0014_docker_endpoints.sql`);
- a systemd drop-in (`docker.service.d/forge-tls.conf`) repeats dockerd's
  own command line and adds `-H tcp://<listen>:<port> --tlsverify`; the
  helper rolls it back when dockerd does not come back.

Disabling (`docker.endpoint.disable`: `remove`, `forget`) removes the
drop-in and deletes the CA. Calls are deny-by-default against a catalog of
108 Engine API endpoints (`forge-docker`), each with a class mapped to an
**existing** action: read `server.manage`, manage `container.manage`, exec
`container.exec`, admin `server.command` (MFA). A body guard raises
container, exec and volume creation with host access to admin.
`Config.Env` in container inspect is redacted without
`container.inspect_env`. Calls outside the read class are audited as
`docker.api` with op, method, path, query keys, status, class and
host-access reasons, never bodies.

**Why.** The user chose direct TCP with mutual TLS over relaying through the
agent link, to offer the whole Engine API, as Portainer and Swarm tools
expect, with streaming and upgraded connections, without re-implementing
each call as an agent command.

**Consequences.**
- The node has a root-equivalent port open on the network. A per-node CA
  limits the blast radius to that node; operators should limit the port
  with `listen` and a firewall to the control plane only. Whoever holds the
  master key and the database can reach every enabled node's dockerd.
- The existing agent-socket path (Containers page, stacks, container shells)
  is unchanged and needs no open port.
- No new Cedar actions; the classes reuse existing ones.
- `docker_tls` is a new agent command kind, offered automatically to agents
  after an update (not opt-in).
- Enabling and disabling restart dockerd; without `live-restore`, running
  containers stop, so enabling needs `acknowledge_restart` when they run.
- Linux nodes with systemd only; Windows nodes are not supported.

## D-126 YAML deployments and release orchestration of services (issue #73)

**Decision.** A `forge/v1` `Deployment` document (`forge.deploy.yaml`)
describes one release of one **service** (the API's `application`) in one
environment: runtime, artifact, template fields, plain `env`, secret
*names*, placement, strategy, health gate, draining, rollback and
per-environment `overrides` (JSON merge patch). Applying it creates an
immutable, numbered **release** (spec, env with sealed secrets, artifact,
policy, the YAML and its SHA-256) and a **deployment** that rolls it out
(migration `0015_releases_deployments.sql`).
- **The YAML wraps a Service.** It creates or updates an existing service;
  there is no second kind of deployable object. The console's **Deploy**
  button and `POST /applications/{id}/deploy` keep deploying in place.
- **Slots on pooled ports, not in-place swaps.** Start-first strategies run
  the new release next to the old one in slot `a`/`b` (its own container or
  unit, `forge-app-<ns>-<app>--<slot>`) on host ports from 20000–29999,
  allocated per service and slot in `deploy_ports` and unique across
  services. Stop-first and recreate deploy into the serving copy. A
  pre-existing service is the unslotted instance until its first slotted
  rollout.
- **A pure planner** in `forge-catalog` turns the policy and each node's
  serving copy into steps (`start`, `gate`, `route`, `wait`, `approve`,
  `stop`) and refuses unsafe plans: offline nodes, slots with no endpoint
  routing to them, jobs in slots, and full-fleet stop-first without
  `allowDowntime`.
- **One `deploy.rollout` job with per-step durable progress.** The plan is
  stored on the deployment and `step` counts steps done, so a resumed job
  continues where it stopped; every step is idempotent. A
  `deployment_locks` row keeps one active deployment per service (`409`).
  Pause, resume, abort, approve and reject are requests on the deployment
  that the job reads between steps.
- **Edges re-applied as their last applier.** A `route` step writes slot
  weights and re-applies every endpoint routing to the service as whoever
  applied it last, as certificate renewals do; the resolver expands a
  service to its weighted copies (weight 0 drained) and sites gained
  `weights` (Nginx `weight=`).
- **Probe-now agent action.** Gates send the node a `probe` app action
  (default check plus HTTP/TCP readiness checks of the workload, retargeted
  to the slot's ports) instead of waiting for the health cycle; `retire`
  stops and removes one copy; `stage` fetches a pinned Git commit or a
  SHA-256-checked archive into `<home>/releases/<n>` as the app's account.
- **Health revision stays logical.** The node's health runner follows the
  serving slot, but the policy revision pushed is the service's own; the
  slot form is derived from it, so a node never reports an unknown revision.
- **Traffic log fields.** The edge traffic log line gains
  `us=$upstream_status rt=$upstream_response_time`, giving per-upstream
  5xx counts and p95 latency for canary analysis. Older lines without them
  still count as requests.
- **Four-eyes approvals.** A new Cedar action `deploy.approve`
  (`platform_admin`, `workspace_admin`, `operator`; not `developer`) gates
  approvals and rejections, and never on one's own deployment.
- Tag-only images are pinned to the digest the first node pulls, recorded
  on the release.

**Why.** CI needs a declarative, reviewable file and a single command that
exits with the rollout's outcome; zero-downtime rollouts need two copies on
one node, and Forge already owns the edges that can shift traffic between
them. A durable, step-indexed job fits the existing job engine and survives
control-plane restarts mid-rollout.

**Consequences.**
- Slotted strategies need a Forge endpoint, and edges must reach nodes on
  TCP 20000–29999. Native services in a slot must listen on `$PORT`.
- Rollback restores definition, artifact and env (including the secret
  values sealed in that release); it does not undo data or schema changes.
- Canary analysis sees only traffic through Forge endpoints.
- Linux nodes only; nodes need an agent that understands `probe`.

## D-127 Two databases: operational and analytics

**Decision.** Forge has an **operational** database (SQLite or PostgreSQL,
default name `forge_op`) and a separate **analytics** database for node
history and application logs (chDB, ClickHouse, PostgreSQL or off, default
name `forge_op_analytics`). Analytics engines today: `embedded` and
`clickhouse`; `postgres` is reserved.
- `forge.toml` gets `[database]` (`url` without database and password,
  `name`, `password_file`) and `[analytics]` (`engine`, `url`, `name`,
  `password_file`, `retention`). `database_url` keeps working and wins when
  set. A password inside a URL is refused; it comes from `password_file` or
  `FORGE_DATABASE_PASSWORD` / `FORGE_ANALYTICS_PASSWORD`, and messages show
  URLs with `***`.
- `forge-server db check [--create] [--allow-shared]` classifies the target
  as missing, empty, Forge, newer Forge or foreign and exits 4 when Forge
  must not use it. `init` creates a missing PostgreSQL database. Forge never
  drops or recreates a database; a database with other tables needs
  `--allow-shared`.
- The installers ask for both databases (`--db`, `--db-url`, `--db-local`,
  `--db-name`, `--allow-shared-db`, `--analytics`, `--analytics-url`,
  `--analytics-name`). `--db-local` installs PostgreSQL from the system
  packages and creates a `forge` role with a generated password.
- PostgreSQL support is in every `forge-server` build (`postgres` is now a
  default feature); before, only CI built it.

**Why.** Logs and metrics grow without bound and are read as ranges and
aggregates. In the transactional database they would slow backups, vacuum
and every operational query. ClickHouse (and chDB, its embedded form) stores
the same data 10–20× smaller with cheap TTL expiry, and both use one SQL
dialect, so moving from chDB to a ClickHouse server is a data copy, not a
code change. Operations must keep working when analytics is down, so the two
never share a transaction.
- **Embedded engine = the ClickHouse binary, run by Forge.** The owner chose
  "separate download" for chDB. chDB is a C library; linking it needs FFI
  (`unsafe`, denied workspace-wide) and puts hundreds of MB in the server.
  Running the official ClickHouse binary as a supervised child process on
  `127.0.0.1` gives the same engine and SQL with no `unsafe`, keeps
  `forge-server` small, and isolates crashes and memory. `chdb` is accepted
  as an alias of `embedded`. `forge-server analytics install --binary
  --sha256` installs the binary; the engine gets a generated password, a
  memory cap (`embedded_memory_mb`), two background threads and no system
  logs.
- **Node history.** Heartbeats carry `top_processes` once a minute
  (`NodeMetrics` tag 15). The server folds samples per node and minute and
  writes once a minute (`node_metrics_1m`, `node_top_processes_1m`, hourly
  `node_metrics_1h` through a materialized view, TTLs from `[analytics]
  retention`). A bounded buffer holds rows while analytics is down.
  `GET /servers/{id}/history`, `/servers/{id}/report` and
  `/analytics/status`; CLI `forge nodes history|report|analytics`.

## D-128 Seq-compatible application logs

**Decision.** Applications send their logs to Forge with the Seq ingestion
API, so existing Seq clients (Serilog, NLog, MEL, seqlog, pino-seq and
others) work by changing only the server URL and the API key.
- `POST /api/events/raw` accepts CLEF lines or the older
  `{"Events":[...]}` format, authenticated by the `X-Seq-ApiKey` header,
  and answers 201 `{"MinimumLevelAccepted":null}`.
- Each ingest key belongs to one application (Service). Tokens start with
  `flk_`, are shown once, and only their SHA-256 is stored
  (`log_ingest_keys`, operational database). Creating and revoking keys
  needs `log.manage` and is audited.
- Events go to `app_logs` in the analytics database, ordered by
  `(application_id, ts, id)`, with three timestamps: `ts` (the app's),
  `node_ts` (reserved for agent ingest) and `ingest_ts`, from which the
  retention (`app_logs_days`, default 30) counts.
- The console reads with `log.read` (not customers: logs often hold
  personal data). Search is free text plus `Name="value"` property filters,
  compiled to parameterized SQL; raw SQL is never accepted. Pages are
  newest first with `(ts, id)` cursors; the live tail polls with `after`.

**Why.** Seq's protocol is the most widely supported structured-log
target, so no SDK per language is needed. Ingest is a pass-through that
writes one batch per request: when analytics is down the server answers 503
and the sender retries, so nothing large is buffered in the server.

**Not yet.** The agent-side endpoint with a disk spool (logs survive a
server outage on the node), stdout/journald collection, OTLP, the Seq
filter language beyond property equality, saved searches, and per-key
rate limits.

## D-129 Removing a node deletes it; no soft delete

**Decision.** `DELETE /servers/{id}` (console **Remove**, `forge nodes
remove`) revokes the node's certificate and then hard-deletes the node and
everything recorded for it in one store transaction
(`ServerRepository::delete_server`). It takes the same confirmations as
revoke (exact hostname, MFA code when required) and `server.manage`; the
control node cannot be removed.

- Deleted: facts, logins, SSH, Docker endpoint, file roots, uploads and
  grants, installations, app instances, stack parts, deploy slots and
  events, object store memberships, tunnels and tunnel grants, observations,
  alarms (with events, transitions and trackers), the DNS label, tags, role
  bindings and profile/group assignments scoped to `server:<id>`. Queued
  jobs on `server:<id>` are cancelled.
- A stack whose default node it was moves to another node it runs on; a
  stack on that node alone is deleted.
- Kept: the audit log (append-only, hash-chained) and the
  `revoked_certificates` row, so the old certificate stays refused after
  the node row is gone.

**Why.** The operator wants a re-enrolled machine to start fresh under the
same name. A revoked node kept its hostname and its `node_names` label, so
the new node got a suffixed DNS name and hostname lookups could match the
old row. Soft delete was considered and dropped: every query over node data
would need a filter, and nothing needs the old rows back.

**Not done.** Nothing runs on the machine: containers and files stay until
the operator removes them (or uninstalls the agent with `--purge`).

## D-130 Cross-platform developer scripts in Python; sccache per machine

**Decision.** The developer and lab scripts (`scripts/dev/*.py`,
`scripts/setup_testing/publish.py`, `scripts/setup_testing/perf/*.py`) are
Python 3 with the standard library only, replacing their PowerShell versions.
They pick native or WSL builds, `.exe` suffixes and the SSH askpass helper by
platform. sccache is turned on once per machine with `rustc-wrapper` in the
user-level `~/.cargo/config.toml`, sharing one cache across every Rust project
on the machine; `scripts/dev/build_env.py` reports the setup and sweeps old
artifacts.

**Why.** Development moved to Linux hosts as well as Windows, and the lab
tooling (`vm_lab.py`, `feed_server.py`, scenarios) was already Python. A Python
process cannot change its parent shell, so a per-machine Cargo config replaces
the dot-sourced `build-env.ps1`.

**Not changed.** `install.ps1` (node and control-plane installers) and the
PowerShell catalog recipes stay PowerShell: they run on Windows nodes, not on
developer machines. The repository's `.cargo/config.toml` still never sets
`rustc-wrapper`, because CI and `cross` builds have no sccache.

## D-131 Public website and docs at forge.orbyntech.com; free binaries, private source

**Decision.** `website/` holds the public website: React 19 + TypeScript +
Vite + Tailwind v4, the same stack as the console, with every page
pre-rendered to static HTML at build time by a small script
(`website/scripts/prerender.mjs`), not a framework. The Starlight docs move
under `/docs/` (`base: "/docs"`; content links carry the prefix). One image
(`website/Dockerfile`, unprivileged nginx) serves both, and
`.github/workflows/website.yml` pushes it to GHCR. The brand lives in one
file, `website/src/styles/tokens.css`, which the docs import and map onto
Starlight's variables.

Releases stay in the public `orbyntech/forge-releases` repository
(`RELEASE_REPO`). Forge is distributed under the Orbyn Forge Free Use
License (`scripts/packaging/LICENSE.txt`): free for any use, including
commercial use, with no resale and no modified redistribution. The license
ships in every package and as a release asset. The source repository stays
private until it is opened.

**Why.** One origin gives one design, a shared theme choice and links from
the website straight into the docs. Pre-rendering matters for a public site:
a client-only single-page app shows crawlers and link previews an empty shell.
Reusing the console's stack adds no new dependency families. The only
addition is `@astrojs/sitemap` in the docs, which was already in the
lockfile.

**Not done.** The workspace `Cargo.toml` still says `license =
"Apache-2.0"`, and the release repository's `README.md` and `LICENSE`
(`scripts/packaging/forge-releases/`) are copied there by hand.

## D-132 RHEL-family parity checks and arm64 in CI

**Decision.** RHEL, Rocky and Alma (8 and 9) are first-class Linux targets,
and arm64 (`aarch64`) Linux builds run in CI, not only at release time.
- The control-plane installer's `--db-local` installs the PostgreSQL server
  (not just the client), enables the `postgresql:15` stream on EL8,
  initialises the cluster, and switches local TCP logins in `pg_hba.conf`
  from `ident` to `scram-sha-256`. Without that, Forge's password login to
  its own database is refused on RHEL-likes.
- Both installers run `restorecon` on the installed binaries, so SELinux
  labels stay correct even under a custom `--prefix`.
- The agent opens the WireGuard port in firewalld when firewalld runs, and
  retries the `wireguard-tools` install after `epel-release` on dnf systems.
- `scripts/packaging/test-install.sh rockylinux:9` (with `DB=1` for
  `--db-local`) is the RHEL install test.
- The CI static musl job is a matrix of x86_64 and aarch64 (cross-compiled
  with `cross`), and runs the arm64 binaries under the QEMU in the cross
  image. `publish.py --arm` builds the arm64 assets for the local feed.
- `cross` builds use their own target directory: build scripts compiled by
  the host toolchain need a newer glibc than the cross image has.

**Why.** The docs already listed RHEL and arm64 as supported, but nothing
tested either. The PostgreSQL gap would have broken every `--db-local`
install on RHEL.

**Not yet.** arm64 is smoke-tested (`--version` under QEMU), not covered by
the test suite. The App Store recipes' dnf branches are not exercised in CI.

## D-133 Release archives are hashed in a stream, not in memory

**Decision.** A native release archive is still downloaded by `curl` as
the app's account, now under `prlimit --fsize`, so the 2 GiB limit holds
while the file is written even when the server sends no Content-Length.
The agent then hashes the file in 64 KiB chunks
(`self_update::sha256_nofollow`, link not followed, inside
`spawn_blocking`) instead of reading it whole into memory, and compares
the SHA-256 before anything is unpacked. Agent self-update uses the same
helper. Unpacking is unchanged: `tar` or `unzip`, as the app's account.

**Why.** Reading the archive into a `Vec` could make an agent that idles
at about 17 MB use up to 2 GiB of RAM, and it blocked the async runtime.

**Not done.** The design for deployment sources proposed unpacking with
the file explorer's extractor to drop `tar` and `unzip`. That extractor
writes every file as `0644` and skips all links. Release archives need
exec bits (Go binaries, scripts) and in-tree links (`node_modules/.bin`),
so switching now would break working deployments. It can be done once the
extractor keeps modes and links that stay inside the tree.

## D-134 MFA is each user's choice by default

**Decision.** This revises D-064. `[security] require_mfa` now defaults to
`false`. MFA is each user's choice: a user who enrolls TOTP or a passkey is
always asked for it at sign-in, but no operation asks for MFA or step-up;
browser sessions and API keys count as MFA-verified for guarded actions.
Connector sessions still never do (D-063). The installer no longer asks
"Harden with MFA?"; new installs write `require_mfa = false`, and the
elevated-tokens question is asked only with `--mfa required`. The console
banner and the start-up warning/error are removed; the server logs an
`MFA policy` info line instead, and `forge-server init` no longer warns.
Opt-in stays: `forge-server mfa on`, `FORGE_REQUIRE_MFA=true` or
`install.sh --mfa required` restore step-up for guarded actions, where
`allow_elevated_tokens` applies again. Existing installs keep their
setting: an upgrade does not touch an existing `require_mfa = true`.
Capabilities `security.mfa_required` and `dev.skip_mfa` are unchanged.

**Why.** The owner wants MFA to be a personal sign-in choice that never
blocks operations. Operators who need the guardrail opt in; the hardening
guide recommends it for production.

## D-135 Log ingest is batched across requests
**Decision.** `POST /api/events/raw` no longer writes each request with its
own `INSERT`. Handlers parse and attribute events as before, then queue them
for one writer task per server (`log_batcher`), which takes the queued
requests, lingers up to 100 ms for more, and writes them in one insert of at
most 20,000 rows. Each request waits for its batch and is answered after the
insert: `201` still means stored, and a failed insert answers every waiting
request with `503`. The queue holds 4,096 requests; when full, ingest answers
`503` `ingest_busy` at once. Revises the pass-through ingest of D-128.

**Why.** A load test (debug build, local ClickHouse) with 200 concurrent
senders failed 88% of requests with `TOO_MANY_SIMULTANEOUS_QUERIES` and
created one part per request (209 parts for 1,000 requests). With batching
the same test stored every event, p99 latency fell from 17.9 s to 1.4 s,
and the table kept a handful of parts. Answering after the insert keeps
senders' retry logic (Serilog's durable sink) correct; nothing is held in
memory past the answer, so a crash loses no acknowledged event.


## D-136 Analytics retention and resource limits change at runtime
**Decision.** Administrators change analytics settings under
**Administration → Settings → Analytics** (`GET`/`PUT
/api/v1/analytics/settings`, Cedar action `analytics.configure`, platform
admins, under the MFA guardrail). `forge.toml` gives the defaults; overrides
are stored in the operational database (`analytics.overrides`) and
published on a `watch` channel, so nothing needs a `forge-server` restart.
- **Retention** per stream: minute metrics, top processes (new, separate;
  defaults to the minute-metrics value so upgrades keep their behaviour),
  hourly rollups, logs. Applied with `ALTER TABLE .. MODIFY TTL` and
  verified by reading the TTLs back. Shortening needs confirmation.
- **Managed engine CPU and memory** (cgroup part superseded by D-140): a CPU quota (vCPU) and an optional hard
  memory limit through a cgroup v2 of the engine's own. The unit gets
  `Delegate=cpu memory`; Forge moves itself into `<service>/server` and the
  engine into `<service>/analytics` and writes `cpu.max`/`memory.max`. A
  limit below current use needs confirmation. The engine's own memory budget
  (default 1,024 MiB) rewrites its `config.xml`; when the engine does not
  pick it up within 15 s the setting is "pending restart", and the
  administrator restarts the engine now or at a scheduled time.
- **Search** (searches at once, timeout, lookback, threads and memory per
  query), **ingest** (queued bytes, rows per insert, flush interval) and
  **disk protection** (minimum free disk, read from `system.disks`). Ingest
  answers a retryable `503` when the queue's request or byte limit is reached
  or disk is low; search answers `503` `search_busy` over its limit.
- Each change reports, per setting, `applied`, `pending`, `pending_restart`,
  `failed` (with the reason) or `external`, and is audited with old and new
  values. For an external ClickHouse, CPU, memory and the memory budget belong
  to its host and are reported `external`.

**Why.** Retention and limits depend on the host and the data volume, which
change after install; editing `forge.toml` and restarting interrupts the
control plane. Verifying every value against the engine or kernel, instead of
assuming the write worked, shows partial failures (missing delegation, missing
table permissions) where the administrator made the change. Cgroups give an
OS-enforced cap without `unsafe` or new dependencies; the engine stays a child
process (D-127), so a kill only restarts analytics.

## D-137 Every Docker container's logs and metrics are collected

**Decision.** The root helper follows the output of every running Docker
container on a node (Forge services, stacks and unmanaged containers) and
samples each container's resources once a minute. The agent polls it with a
new closed helper request, `container_telemetry`, before each heartbeat and
sends the lines (`Heartbeat.container_logs`) and samples
(`NodeMetrics.containers`) to the server. Collection is on by default; a
node setting (`node.<id>.container_telemetry`: `enabled`, `exclude`) turns it
off or leaves containers out by name or `key=value` label, and is sent back
in every heartbeat answer. While analytics is off the server tells nodes not
to collect.

- **Delivery.** The helper keeps one batch pending until the server
  acknowledges it (`HeartbeatResponse.container_logs_ack`, set only after
  the lines are stored). Only an acknowledged batch moves the per-container
  cursor (Docker timestamp, ns), kept in the helper's root-only state folder
  rather than the agent's, so the agent user cannot plant a link there.
  Restarts of the helper, agent or server neither drop nor repeat stored
  lines; a batch whose acknowledgement is lost is stored twice. The buffer is
  bounded (20,000 lines, 16 MiB), drops the oldest and reports the count.
- **Attribution** is done by the server from the authenticated link:
  `forge.app = <environment>/<name>` names a service only if it is placed on
  that node; `forge.stack`, else the compose project, names the stack;
  anything else is unattributed and readable with `view` on the node.
- **Storage.** Lines go into `app_logs` (now ordered by `(server_id, ts, id)`
  with a skip index on `service_id`, `source = ingest | container`); samples
  into `container_metrics_1m` and the `container_metrics_1h` rollup, with the
  node-metrics retentions.
- **Unsigned request.** `container_telemetry` is allowed without a signed
  command (an exception to D-068 like `container_state`): it reads only, and
  everything it reads goes to Forge through the agent anyway; its settings
  can only make collection smaller.

**Why.** Operators expect every container's output and resource use in one
place without configuring each app; the live follow that existed only works
while someone watches. Acknowledged batches with cursors give at-least-once
delivery without a disk queue on the node.

## D-138 One vocabulary: Project → Environment → Service

**Context.** D-102 kept the old storage names and changed only the console
labels: a product was an `app_group` (*Application* in the console), an
environment was a `namespace`, and a service was an `application`. Every
reader had to translate between three words for each level, the CLI had
`forge applications` for products and `forge apps` for services, and the
docs needed a mapping table on every page. A service could also be a `job`
(a systemd timer), which shared nothing with the durable jobs engine but
its name.

**Decision.** The levels are **Project → Environment → Service** in every
layer: tables (`projects`, `environments`, `services`), REST paths
(`/api/v1/projects`, `/api/v1/environments`, `/api/v1/services/…`), fields
(`project_id`, `environment_id`, `service_id`), Cedar types (`Project`,
`Environment`, `Service`), access scopes (`project:`, `environment:`,
`service:`), audit actions (`project.*`, `environment.*`, `service.*`), alert
and silence scopes (`services`, `environment`), the CLI (`forge projects`,
`forge environments`, `forge services`, `--environment`, `--service`) and
the console (**Projects**, the project explorer). Environment names are
free text, unique per cluster. A service's runtime is `systemd` or
`container`; the `job` runtime, its `schedule` field and the `run` action
are removed. This is a hard break with no aliases. This *project* is new
and unrelated to the projects that D-056 removed.

- `forge tunnel --app` and the ingress target kind `app` keep their names:
  they are wire names for a service target, not the level's name.
- Unit and container names on the nodes stay `forge-app-<environment>-<service>`,
  so running workloads keep their names.

**Why.** One word per level removes the translation everywhere, and only a
release that already requires a fresh database (D-139) can rename storage,
routes and policies without a migration path. Long-running services keep
health checks, placement and ingress simple; periodic work runs inside a
service or an external scheduler.

## D-139 One baseline migration; fresh database for this release

**Context.** The store had accumulated many migration files, several of them
only renaming or reshaping what earlier ones created, and the analytics
schema had its own version history. Renaming the core tables (D-138) would
have added another large migration on top.

**Decision.** The SQL migrations are squashed into one baseline,
`0001_initial.sql`, and the analytics schema restarts at version 1. Every
later change is a new migration file after the baseline; a released one is
never edited. Upgrading a database or analytics store made by an earlier
build is not supported: this release starts on an empty data directory.
`reference/upgrade-notes.md` states it.

**Why.** One baseline makes the schema readable in one file, and a release
that breaks the API and the CLI anyway (D-138) is the cheapest point to drop
the upgrade path. Doing it later would mean writing and testing a real
migration from every earlier schema.

## D-140 Engine limits without cgroups

**Context.** D-136 capped the embedded analytics engine through a cgroup v2
subtree: Forge moved itself into a `server` leaf and the engine into an
`analytics` leaf. That needed `Delegate=cpu memory` in the unit, failed in
a login session or any unit without it ("Forge may not manage its cgroup"),
and put `forge-server` itself under a cgroup Forge managed.

**Decision.** The limits apply to the engine only and need no privileges.
- **Memory:** the memory budget (`memory_budget_mb`) is ClickHouse's
  `max_server_memory_usage`, which the engine enforces itself. The hard
  memory limit (`memory_limit_mb`) is removed: without a cgroup nothing
  unprivileged could enforce it (`RLIMIT_AS` breaks an engine that reserves
  address space). Stored overrides drop it; `forge.toml` still parses it and
  warns.
- **CPU:** `cpu_quota_vcpu` is a hard cap by CPU affinity, rounded up to
  whole CPUs and never more than Forge may use. Forge spawns the engine
  from a short-lived thread that pins itself with `rustix`'s safe
  `sched_setaffinity` (the `thread` feature of a crate already in the
  tree), so the engine inherits the mask from its first instruction, no
  `unsafe` `pre_exec` is needed, and no `forge-server` thread that keeps
  running is ever pinned. A change re-pins every thread of the running
  engine through `/proc/<pid>/task`. `max_threads`,
  `concurrent_threads_soft_limit_num` and the background pool sizes follow
  the cap; the pools resize when the engine restarts.
- **Usage** (CPU and resident memory) comes from `/proc/<pid>/stat` and
  `/proc/<pid>/status`; the pinned CPU count is read back with
  `sched_getaffinity`. The embedded engine stays Linux-only; on other
  systems pinning reports itself unsupported.
- **External ClickHouse:** CPU and memory settings are not Forge's to set.
  `PUT /api/v1/analytics/settings` refuses them with `409
  managed_externally` (a reset with `null` is allowed), the console hides
  the section, and the `external` apply status is gone.
- The installer no longer writes `Delegate=cpu memory`.

**Why.** Affinity and the engine's own memory cap work for any user, in
any unit, and touch only the engine. Rounding up to whole CPUs is coarser
than a CFS quota, but a hard cap that always applies beats a precise one
that needs delegation. A setting that cannot be enforced is removed rather
than kept as a no-op.

## D-141 The analytics worker process

**Context.** The capacity benchmark broke one server between 1,000 and
2,000 nodes. The backlog sat in `forge-server` memory: the log batch
queue, request bodies plus their decoded events, a second copy during the
insert, and heartbeats held while their container lines were stored. A
container batch whose heartbeat timed out was also written later and then
sent again, so it was stored twice. Analytics load could take the control
plane down with it.

**Decision.** All analytics work moves into one process,
`forge-analytics-worker` (`src/apps/forge-analytics-worker`), a child of
`forge-server`. `forge-server` becomes its client and supervisor and keeps
no analytics data, buffer or queue.

- **What the worker owns:** the analytics store (`AnalyticsStore`; the
  ClickHouse adapter or a bounded in-memory store for tests), node-minute
  and container buffers and their flush, the log batch writer, CLEF
  parsing and attribution, log searches and history reads, retention and
  runtime tuning apply, usage sampling, and the embedded engine as its own
  child. The worker is the analytics composition root (`wiring.rs`): only
  there is a store chosen; everything else takes the port.
- **What `forge-server` keeps:** ingest keys and their lookup, the node
  collection settings, the settings overrides with validation,
  confirmation and audit, the scheduled engine restart, permission
  filtering of every log query, and the maps the worker attributes with
  (active nodes, `<environment>/<name>` → service per node, placements with
  node names, ingest key hashes). The maps are rebuilt every 15 s from four
  batch reads and pushed when their hash changes and on every worker
  (re)connect. API handlers authorize as before and call the worker through
  two `forge-core` ports, `AnalyticsReader` and `TelemetrySink`; admin
  calls whose types carry OpenAPI schemas stay a server-side client API.
- **Control protocol** (`forge-analytics-proto`): a Unix socket in
  `<data_dir>/analytics-worker/` (directory 0700, socket 0600; loopback
  TCP on Windows). Frames are `u32 header length | u32 blob length |
  JSON header | blob`, with caps checked before allocation (requests 16
  MiB each, responses 32 MiB). The blob carries opaque payloads, so the
  server forwards CLEF bodies and re-encoded heartbeat protobufs without
  parsing them. Every connection starts with `Hello { secret, protocol }`;
  the secret is 32 random bytes handed over on the worker's stdin at each
  start and never written to disk, and is compared in constant time. A
  protocol mismatch marks the worker `incompatible` instead of restarting
  it in a loop. The server keeps a pool of at most 16 connections, a
  deadline per request kind, and drops any connection that failed or timed
  out; health pings use a connection of their own.
- **Supervision:** spawned with `kill_on_drop`; it exits when its stdin
  closes, so it never outlives `forge-server`. Restarted with back-off
  (1 s doubling to 60 s, reset after 5 minutes up) when it exits, misses
  three pings 5 s apart, or its RSS passes `[analytics]
  worker_memory_limit_mb` (default 2048, `0` = none). Each restart is
  audited as `analytics.worker.restart` and shown in the `worker` object
  of `/analytics/status` and `/analytics/settings`. While it is down,
  analytics endpoints answer `503 analytics_worker_down`. A stale embedded
  engine left by a killed worker is found by its pid file and stopped, but
  only when `/proc/<pid>/cmdline` names the engine binary.
- **Bounded, and no double write:** the batch writer's byte cap counts the
  decoded size of events; a full queue answers `Busy` at once and keeps
  nothing. A queued batch whose waiter has gone (the server dropped the
  connection at its deadline) is skipped before the insert. Container
  batches are remembered by `(node, seq, line count, first and last
  timestamp, dropped, bytes)` in an LRU of 8,192 entries, so a batch sent
  again after a lost ack is acknowledged without a second write.

**Consequences.** A worker restart loses the node samples of the open
minute; queued log batches fail and their senders retry. Seq/CLEF ingest
is at least once: a sender whose request timed out while its insert ran
stores the batch again on retry. The container dedupe lives in memory, so
a restart between storing and acknowledging can still store one batch
twice. Both close with the agent spool and its file ids (next step of the
plan). Heartbeats still carry telemetry in this step and the server
re-encodes it once per heartbeat. Release packages and the self-updater
ship the new binary next to `forge-server`; no new systemd unit.

## D-142 Agent spools and direct upload to the analytics worker

**Context.** After D-141 the worker owned all analytics work, but every
node still sent its container lines, container samples and top processes
in heartbeats, `forge-server` re-encoded and forwarded them, a container
batch could still be stored twice after a worker restart, and apps on a
node sent CLEF logs to `forge-server` over the network. Analytics load
still passed through the control plane.

**Decision.** Each agent keeps a disk spool and uploads it straight to
the worker.

- **File format** (`forge_core::spool`): zstd-compressed NDJSON named
  `<node-id>-<file-id>.ndjson.zst`, the file id a UUIDv7 in 32 hex
  digits (time ordered). The first line is a header `{"v":1,"node",
  "created","counts"}`; each later line has a kind in `"k"`: `log` (one
  request body to the local listener, unparsed, with the hex SHA-256 of
  its key), `container_log`, `node_minute`, `top_process`,
  `container_minute`. The node id in the file is never trusted.
- **Spool** (`forge-agent/src/spool.rs`): rows go into one write buffer,
  sealed at 4 MiB raw or 10 s into `.tmp` → fsync → rename `.ready` →
  directory fsync. Upload is oldest first, one file at a time; a file is
  renamed `.acked` and deleted only after the worker's ack names it.
  Start-up removes `.tmp` (may be partial) and `.acked` files and drops
  ready files past their age. Limits: 512 MiB cap (the oldest file is
  dropped and counted), 256 MiB free-disk floor, 72 h maximum age, zstd
  level 1 (1 to 3 allowed). Container lines are sealed before the helper
  is acknowledged, so the helper's cursor never passes lines that are
  not on disk. Lines in the write buffer at a crash are lost (at most
  the seal interval of app logs). Tuning comes from `[analytics.spool]`
  in heartbeat responses. `zstd` adds about 0.55 MB to the agent binary
  (measured on the release build), within the 5 MB rule.
- **Local listener** (`log_listener.rs`): Seq-compatible `POST
  /api/events/raw` on `127.0.0.1:5341` and on the Docker bridge gateway
  (private IPv4 only, read from the engine by the helper), never a
  wildcard. `ListenerTuning.extra_address` is the hook for a mesh
  address. `X-Seq-ApiKey` is hashed and compared in constant time with
  the hashes of the keys of services placed on this node, sent in
  heartbeat responses. A per-key token bucket (50/s, burst 100) applies
  off loopback by default. The body is capped (1 MiB) and appended
  without parsing; a full spool or a disk below the floor answers 429.
  Deployed services get `FORGE_LOG_URL` (`http://127.0.0.1:5341` on the
  host, `http://forge-node:5341` plus `--add-host
  forge-node:host-gateway` in containers); a value the app sets wins.
- **Upload listener** (worker `ingest/upload.rs`): HTTP/2 over mTLS on
  `[analytics] worker_agent_listen` (default `0.0.0.0:55132`). Client
  certificates are required and verified against the Forge CA like the
  agent hub; the node must be in the pushed active-node list, and the
  file name must name the same node. The certificate and key are issued
  by `forge-server` at each worker start and handed over on stdin, never
  written to disk. Agents learn the URL (`[analytics] worker_agent_url`,
  default the agent URL's host with the worker port) at enrollment and
  in every heartbeat response.
- **Import** (`ingest/spool_file.rs`): the worker decompresses the file
  as a stream (64 MiB decompressed cap) and parses rows in Rust:
  application bodies with `parse_batch` and container lines with
  `container_event`, the same functions as before, so a line is stored
  the same way whichever path it took. `server_id` is always the
  certificate's node, a key counts only for a service placed on that
  node, and a `forge.app` label only names a service placed there;
  other app rows are skipped and counted. We chose this over a
  `INSERT … SELECT … FROM input()` that parses in the engine: matching
  `container_event` and the trust rules in SQL would be a second
  definition of both, and drift between them would store the same line
  differently. Rows are written with one insert per table, zstd-encoded
  (`Content-Encoding: zstd`), with `async_insert=1,
  wait_for_async_insert=1`: the engine groups the many small inserts of
  thousands of nodes into larger parts, and still answers only once the
  rows are written, so the ack follows the store. We preferred this to a
  grouping buffer in the worker, which would hold rows in worker memory
  and delay acks.
- **Once:** stored file ids are kept in `<data_dir>/analytics-worker/
  spool-ids/seen.log`, the last 64 per node (an agent only resends its
  oldest unacknowledged file), compacted at 200,000 lines and when nodes
  leave the fleet. This replaces the in-memory container dedupe for
  spooling agents and holds across worker restarts. A failure between
  two table inserts makes the agent resend and stores the written part
  twice (at least once).
- **Load:** at most `[analytics] worker_max_uploads` imports (default 8, 8 to 32; more slots use more network)
  run at once; one more is answered 429 at once and nothing is held. The
  agent backs off with jitter and the spool absorbs the backlog.
- **Heartbeats:** an agent that spools sets `Heartbeat.spool` (its
  status) and sends no `container_logs`, `NodeMetrics.containers` or
  `top_processes`; the server then forwards nothing. Old agents are
  still forwarded as in D-141 for one release.
- **Off-fleet apps:** `POST /api/events/raw` on `forge-server` stays as
  in D-141 (key checked, the body forwarded to the worker unparsed).
  Moving it to the worker's public listener would add a second
  internet-facing listener with its own TLS and limits; keeping the
  proven path was the lower risk.
- **Control socket:** a socket path longer than 100 bytes is reached on
  Linux through `/proc/self/fd/<dir fd>/control.sock` by both sides, so
  the directory and socket modes still apply.

**Consequences.** Analytics data no longer passes through `forge-server`
for spooling agents. Port 55132/tcp must be reachable from the nodes.
The worker parses rows (CPU in the worker, not the server). A revoked
certificate of an active node is not checked by the worker (the node
must be revoked); the server certificate of the listener is valid 90
days from the worker's start. Stack (compose) containers do not get
`FORGE_LOG_URL` yet.

**Amendment (memory, measured).** Release `forge-server`, memory engine,
fleet-sim soak with the spool path, 1 line/s per node and 5 containers,
5 minutes. Baseline 35 MB. 500 nodes: 97 MB, still creeping. 1,000 nodes:
142 MB and rising. A glibc `malloc_stats` snapshot at 1,000 nodes showed
116 MB taken from the system for 76 MB in use, over 27 arenas. After the
nodes disconnected, it held 120 MB for 19 MB in use, while fds went back to
baseline. So the growth is allocator retention, not a leak. The live
per-node heap is about 60 KB, mostly TLS and HTTP/2 connection state.
Fixes:
- The systemd unit sets `MALLOC_ARENA_MAX=2`. At 1,000 nodes RSS is then
  flat at 114 MB; about 28 MB of that is the mapped binary.
- The live metrics ring is bounded by age (one hour, the console's
  longest range) and gives memory back. At the idle heartbeat rate it
  could otherwise grow to 3,600 samples (about 300 KB) per node.

The 100 MB target is not met at 1,000 nodes (114 MB).

**Amendment (capacity benchmark, `scripts/dev/benchmarks/2026-11-21-chdb-capped-node-scaling.md`).**
With chDB capped at 2 cores and 2 GB, the flow holds about 500 nodes at
20 lines/s per node and falls behind at 1,000; nothing is lost, the spools
absorb the backlog. Two fixes came out of it: the embedded engine now
admits `MAX_UPLOADS + 8` concurrent inserts and 24 reads, limited apart
(a fixed limit of 32 refused imports and caused double writes), and
parameterised statements travel in a form body, because a search scoped
to more than ~1,400 nodes no longer fit in a URL.

## D-143 The DuckDB analytics store

**Context.** The embedded engine (chDB) costs 13–16 % of a core and about
100 MB when idle, plus a 520 MB mapped binary. That is too much for an
install of 2 to 5 nodes with modest logging. Such an install wants
analytics with no separate engine process.

**Decision.** `[analytics] engine = "duckdb"` runs DuckDB inside the
analytics worker. The store is a new adapter crate,
`forge-analytics-duckdb`, linked only into the worker; `wiring.rs` stays
the only place that chooses a store. The default engine does not change.

- **Files.** DuckDB's own format, not Parquet. There is one database file
  per UTC day for each *family* of tables, under
  `<data_dir>/analytics/duckdb/<family>/YYYY-MM-DD.duckdb`:
  - `logs` (`app_logs`)
  - `metrics` (`node_minute`, `container_minute`)
  - `processes` (`top_process`)
  - `rollups` (`node_1h`, `container_1h`), one file per month.

  Each family has its own retention, so a whole file is always deleted at
  once. `meta.duckdb` holds the settings, the event-time range of each
  file, and the rollup bounds. Files are ATTACHed on demand under
  refcounted leases (ATTACH and DETACH act on the whole instance), and
  detached after 10 minutes unused.
- **Ingest day, not event day.** Rows go to the file of the day they
  arrive. A DuckDB transaction may write to only one attached database, so
  this is what lets one batch commit atomically: one transaction per
  family. Queries filter on event time. Each file's event-time range is
  widened in `meta` *before* its rows commit (after a crash it is too
  wide, never too narrow), and a query attaches only the files whose range
  overlaps its window. Rows are appended in time order within a batch for
  zone-map pruning.
- **Exactly once per table.** A new port, `SpoolFileWriter` (forge-core),
  stores one spool file's rows with its id. DuckDB writes the file id into
  the family file's `spool_files` table in the same transaction as the
  rows, and skips any family that already holds the id. A crash between
  two families leaves the file stored in some families; the resend stores
  only the rest. The ids are kept in memory (the last 16 per node and
  family) and are read back from the last 4 days of files at start-up.
  `PerTableSpoolWriter` keeps the D-142 behaviour (at least once) for
  ClickHouse and the memory store.
- **One writer.** All writes go through one thread: spool files, the
  minute buffers, the log batch writer, and the jobs. It gathers commands
  (see D-144 for how) up to `ingest.max_batch_rows`. It then appends them with DuckDB's Appender, in
  one transaction per family. Its queue holds 64 commands; senders wait
  off the async threads.
- **Rollups.** There are no materialized views. The hourly job folds the
  minute rows past each file's bound (`rowid >= upto`; the minute tables
  are append-only) into mergeable hourly states: sums, a count, maxima,
  and the latest identity with its minute. It then moves the bound. A
  crash between the two folds the range again. That doubles every sum and
  count of those hours together, so averages, maxima and identities do not
  change. Late rows add states to their hour. Hourly reads merge the
  stored states with the minute rows past the bound, under a read lock the
  job takes before moving it. So the current hour, and late data, show at
  once, as with ClickHouse's views.
- **Retention.** The hourly job drops files past their family's retention:
  DETACH, then delete the file and its `.wal`. There is no DELETE and no
  VACUUM. Minute files are rolled up first. A leased file is marked and
  deleted at its last release. Retention counts from the ingest day, so
  metrics can stay up to a day plus their spool delay longer than with
  ClickHouse, which counts from event time. Logs count from ingest time in
  both stores. With a retention under 4 days, a file resent after its day
  was dropped is stored again. We accepted this rather than keeping files
  past their retention.
- **Limits.** DuckDB has `memory_limit` = `memory_budget_mb`, whose
  `forge.toml` default for this engine is the new key `duckdb_memory_mb`
  (256), and `threads` = the CPU cap rounded up (2 without a cap). Both are
  applied at once with `SET`. `temp_directory` is under the data dir, and
  extension autoloading is off. A failed autoload crashed the process in
  testing, which is why it stays off. Per-search threads and memory are
  not supported: DuckDB's limits belong to the instance. The search
  timeout interrupts the query, and its slot is held until the query
  stops. `worker_memory_limit_mb` must be at least `duckdb_memory_mb`
  plus 256, or the server refuses to start; otherwise the RSS supervisor
  would restart the worker in a loop.
- **Idle.** Nothing polls. The writer blocks on its queue, and the jobs
  sleep for an hour between runs. The worker's existing 10 s usage sample
  is one cheap `duckdb_memory()` read.
- **Jobs reported.** `rollups`, `retention` and `compaction` report their
  last run, duration, error and work done. They appear in
  `GET /analytics/settings` as `jobs` and on the Analytics settings page.
- **Conformance.** One suite (`forge-analytics-worker/tests/conformance.rs`)
  runs against the memory store, DuckDB, and a ClickHouse server when
  `FORGE_TEST_CLICKHOUSE_URL` is set. It passed on all three, including
  ClickHouse 26.12.
- **Dependency.** `duckdb` 1.20001, MIT; features `bundled` and `json` (the
  property filter needs `json_extract`). It pulls `arrow` 60 (Apache-2.0)
  and 34 other new crates. All are MIT, Apache-2.0 or CC0, with no OpenSSL;
  the one new duplicate is `atoi` 3 beside 2. The audit found no
  advisories. The bundled C++ includes mbedtls, re2 and zstd, all
  permissive. It is linked only into `forge-analytics-worker`, never
  `forge-agent`, but `forge-agent`'s tests build it through their
  dev-dependency on the worker.

**Consequences.** Switching an install's engine starts it with empty
history. The worker binary grows from 6.35 MiB to 57.6 MiB (60,365,112 B,
release, stripped, with `json`), and a clean build adds the bundled C++
compile (798 s on 16 cores for the worker alone). The worker's RSS now includes the analytics data
cache, bounded by `memory_limit`.

## D-144 DuckDB is the default analytics engine, tuned for 100 nodes

**Context.** The benchmark of D-143 (`scripts/dev/benchmarks/2026-11-22-duckdb-vs-chdb.md`)
showed DuckDB idle at 0.0–0.2 % of a core and about 44 MB, against chDB's
13–16 % and about 600 MB with its mapped binary. At 100 nodes × 10 lines/s
it stored every line once, but drained in 34 s with up to 187 files waiting,
and with four console users the log histogram p95 reached 7.8 s.

**Decision.**

- **Default.** `[analytics] engine` defaults to `duckdb`, and `install.sh`
  writes `duckdb` unless `--analytics` says otherwise. Installs that set an
  engine keep it. A `forge.toml` with no `engine` key moved from `off` to
  `duckdb`; the upgrade notes say so. `install.ps1` accepts `duckdb` but
  keeps `off` as its default until DuckDB is verified on Windows. All three
  stores stay supported: DuckDB for small and medium installs, chDB for an
  easy ClickHouse-dialect store with a token index, and ClickHouse for heavy
  load.
- **Group commit.** The writer takes the first command and everything
  already queued, with no timer. A lone upload is written at once. Under load,
  the files that arrive while a batch commits form the next batch. The
  50 ms linger of D-143 is removed: once every upload slot was in a batch,
  it only added latency.
- **Upload slots.** `worker_max_uploads` is now optional. It defaults to 32
  (`MAX_UPLOADS`) with `duckdb` and stays at 8 otherwise. More files in
  flight make larger DuckDB batches and fewer 429s when many nodes seal at
  once.
- **Reads cannot crowd out the writer.** Log searches and histograms get as
  many concurrent DuckDB queries as DuckDB has threads, and history reads
  get the same number in a separate pool, so a slow search never holds up a
  history read. Waiting for a slot counts against the query's timeout. A
  change of the CPU cap resizes both pools.
- **Light-load CPU.** It is the readers, not ingest: 0.5 % of a core for
  ingest at 52 lines/s, against 25 % with four users polling four analytics
  endpoints every 2 s. Each DuckDB query costs 10–45 ms of CPU even on small
  data. Fewer threads help small queries; checkpointing and late
  materialisation did not. The read caps bound this cost; no timer, flush
  or checkpoint loop costs anything at idle.

**Consequences.** Searches over wide windows are the first thing to slow
down on DuckDB: it scans every row of the window, while ClickHouse has a
token index. Above about 100 nodes × 10 lines/s, or with heavy text
searching, use chDB or ClickHouse.

## D-145 Analytics hardening: once per table in ClickHouse, current certificates, agent fds

**Context.** D-142 and D-143 left open items: with ClickHouse (and the
embedded engine) a failure between two table inserts of a spool file
stored the written part twice on resend, and every file was its own small
insert; the worker accepted any CA-signed certificate of an active node,
including one replaced at renewal; its listener certificate was issued
once when `forge-server` started and never renewed; the agent held one
`/proc/<pid>/stat` handle per host process (1,100 to 1,550 fds); the log
listener warned every minute about a Docker bridge address that does not
exist inside a containerised node; and compose stack services did not get
`FORGE_LOG_URL`.

**Decision.**

- **Grouped, deduplicated ClickHouse inserts**
  (`forge-analytics/src/spool.rs`, `ClickHouseSpoolWriter`, a
  `SpoolFileWriter`). One task takes the first queued file and every file
  queued behind it (up to `ingest_max_batch_rows` rows and 256 files, no
  timer, like the DuckDB writer of D-144) and writes one insert per table,
  the four tables concurrently. Each insert is synchronous and carries
  `insert_deduplication_token` = `<file id>:<table>` for one file, or
  `g<sha256 of the node/id list>:<table>` for a group, with
  `async_insert_deduplicate=1` (for profiles that force async inserts) and
  `deduplicate_blocks_in_dependent_materialized_views=1`. Migration 2 sets
  `non_replicated_deduplication_window = 10000` on all six tables: plain
  MergeTree keeps no deduplication log without it, and the rollup tables
  need one so a duplicate block does not reach them through their views
  (checked on ClickHouse 26.12). A failed insert is retried twice with the
  same rows and token (0.25 s, 1 s), so a lost answer is not a second
  write. If a table still fails, the tables each file reached are kept in a
  bounded map (8,192 files), and the resent file is written only to the
  missing tables. Files are answered only after every table they have rows
  in confirmed.
  *Remaining gap:* a worker stop between two table inserts of a group of
  several files; the resends form other groups with other tokens. A file
  stored alone is covered by its token even then. We chose this over
  replaying exact groups after a restart (waiting for every member of a
  group to be resent), which needs durable group state and timeouts.
  *Side effect:* the tables now deduplicate any insert identical to a
  recent one. 26.12 ignores `insert_deduplicate=0` for this (it has
  `deduplicate_insert`, which older servers refuse), so the worker's own
  async inserts are not exempted; their identical blocks are true
  duplicates (log rows carry unique ids; minute tables replace anyway).
- **Current certificate only.** `AnalyticsMaps.node_certs` (serde default)
  carries each active node's `cert_serial`. The upload listener accepts a
  node only with that serial: an allowlist, bounded by the node count,
  instead of pushing `revoked_certificates`, which grows with every
  renewal. Renewal, revoke and remove nudge the maps publisher, so the
  worker learns the change at once rather than within 15 s.
- **Listener certificate renewal.** `forge-server` issues a fresh listener
  certificate at every worker start (it used to reuse the one issued at
  its own start) and, at two thirds of its 90 days, sends a new one with
  `Request::SetAgentTls`. The worker swaps the rustls acceptor used for
  new connections; a pair that does not load is refused and the current
  one stays. Control protocol version 2.
  A renewal failure is retried every 5 s, warned about at most hourly,
  and shown as the worker's `last_error`; with less than a day left the
  worker is restarted (reason `unresponsive`), which hands it a fresh
  certificate. The worker refuses a pushed certificate that is not valid
  now (it does not check the chain: only `forge-server` can send it).
- **Serials in DER form.** The security review found that `forge-pki`
  recorded about 1 serial in 256 with a leading `00` byte that DER drops,
  so the presented serial (`PeerIdentity::serial_hex`) differed: such a
  node could not renew (and, with the allowlist, could not upload), and
  revoking it wrote a serial that never matched. Serials now always have
  bit `0x40` set in the first byte (126 random bits), and
  `forge_pki::canonical_serial` (lower case, leading `00` dropped as DER
  does) is used wherever a stored serial meets a presented one: the maps,
  the worker's check, renewal and revocation (a lookup also tries the
  zero-padded forms recorded before).
- **Agent fds.** The process sampler calls
  `sysinfo::set_open_files_limit(0)`, so no `/proc/<pid>/stat` stays open
  between its once-a-minute reads.
- **Log listener binds.** `EADDRNOTAVAIL` on the bridge address means the
  address is not on this host (a node in a container using the host's
  Docker socket): one `INFO` line, then no retry while the engine reports
  the same gateway. Other bind failures warn once, then retry at 1, 2, 4,
  8 and every 15 minutes with debug lines. Loopback is independent.
- **Stacks.** `StackPart::prepare` adds `FORGE_LOG_URL` and
  `forge-node:host-gateway` to every service unless set (loopback URL and
  no entry with `network_mode: host`; URL only for `service:`/`container:`;
  nothing for `none`).
- **Tests** bind the worker's agent listener on a free port: the default
  `0.0.0.0:55132` collided with a running Forge on the developer machine.

**Amendment (agent memory, measured).** `forge-agent` built with the
`agent` profile, enrolled with a local server (DuckDB), a service's key on
its local listener: 40 s idle, then 20 lines/s for 180 s in batches of 10,
then 60 s idle, on a host with 462 processes. Peak RSS per run:

| Run | Idle | Peak | Anonymous at peak |
| --- | --- | --- | --- |
| default | 10.2 MB | 11.1 MB | 4.1 MB |
| default, again | 11.7 MB | 13.6 MB | 6.7 MB |
| `MALLOC_ARENA_MAX=2` | 10.3 MB | 17.5 MB | 10.5 MB |
| `MALLOC_ARENA_MAX=1` | 10.8 MB | 13.5 MB | 6.7 MB |
| `MALLOC_ARENA_MAX=2`, `MALLOC_TRIM_THRESHOLD_=131072` | 11.3 MB | 12.7 MB | 5.8 MB |

About 7 MB of each figure is the mapped binary. Two default runs differ
by 2.5 MB, more than any allocator setting gained, so none is adopted.
The agent is single-threaded and its buffers are already bounded (one
write buffer shrunk after each seal, one upload in flight). CPU was about
0.6 % of one core under load. Open files: 15 to 16 throughout (they were
1,100 to 1,550 before the sampler fix on a similar host). The helper was
not measured here: a root helper on the developer host would rewrite that
host's sshd configuration (`ssh_principals::ensure`). The 38 MB of
`2026-11-22-chdb-real-agent.md` included the helper's Docker collection
(17.6 MB) and the agent's container path (20 MB); without them the agent
alone is 11 to 14 MB.

## D-146 Forge's own versions: pre-releases below the release, outdated means older (amends D-039, D-098)

**Context.** Forge compared its own versions with the Debian algorithm it
uses for packages, where `1.0.0-rc-01` sorts *above* `1.0.0`: a server on a
release candidate was never offered the final release. Agents were
"outdated" whenever their version differed from the server's, so a
rollout could downgrade an agent that ran a newer build, and the update
job skipped only an exact match. Local dev builds were stamped
`0.1.0-dev.<ts>` (D-039), above the `0.1.0` they precede.

**Decision.**
- `forge_sdk::release::compare_release` orders Forge's own versions
  (server, agent, CLI): numeric core part by part, then a version with a
  pre-release suffix (`-rc-01`, `-rc.2`, `-dev.<ts>`) below the same core
  without one; suffixes compare identifier by identifier (split at `.` and
  `-`; numbers numerically, before words). `+build` is ignored, an empty
  version is the oldest. Packages on nodes keep Debian ordering
  (`forge_catalog::versions`).
- The release list, "update available", the upgrade-only check of
  `POST /system/update/apply` (D-098) and `forge update` use it.
- `forge_server::agent_update_policy` holds `is_outdated` (older than the
  target), `has_reached` and `is_ahead`. `GET /agents/versions` adds
  `ahead`; outdated lists, rollouts, automatic updates and the
  `agent.update` job (skip, await, rollback detection) use these, so an
  agent ahead of the server is never touched.
- Dev builds are stamped with the next patch version
  (`<major>.<minor>.<patch+1>-dev.<yyyyMMdd.HHmmss>`): `local_lab.py`,
  `publish.py`, the MOP, and `dev.py`, which now stamps the server too
  (one stamp per session, so a restart does not rebuild).

**Why.** The owner decided that release candidates and dev builds sort
below the final release (update review, decision 1). Semantic-versioning
order is what operators expect, and "outdated = older" is the only
definition under which a rollout can never move an agent backwards.

## D-147 The agent folder's MANIFEST.json names the version on offer (amends D-037, D-098)

**Context.** The agent update target was the server's compiled-in version,
while the binaries in `agent_dist_dir` were unversioned and unchecked: a
folder left from another build, a hand-copied file or a half-finished
server update offered binaries of one version under the name of another.
The node's dry run caught the mismatch only after the download, and a
server rollback restored the server but not the agent folder.

**Decision.**
- `agent_dist_dir/MANIFEST.json` = `{version, files: {name: sha256}}`.
  `AgentDist::scan` keeps only files the manifest lists with a matching
  hash; an unreadable manifest serves nothing; without one (older
  packages) the builds are the server's version, with a warning.
  `AgentDist::version()` is the update target everywhere (API, jobs,
  automatic updates); `GET /agents/versions` adds `server`.
- Written by `package.sh` (release packages, copied by the installers),
  by the self-updater from the **signed** `SHA256SUMS` (tmp, fsync,
  rename), and by `scripts/dev/dist_manifest.py` (`local_lab.py`, MOP).
- The self-updater swaps `MANIFEST.json` and the folder's `SHA256SUMS`
  like the binaries (`*.prev`) and records every agent-folder file in
  `applied.json` (`dist_files`); the start-up guard's rollback restores
  each `.prev`, or removes a file the update added.
- Rollouts and single updates record `target` in the payload; each child
  `agent.update` fails (fatal) when the version on offer is no longer that
  target, instead of sending other binaries under the old name.

**Why.** The target must be the version of the bytes that are actually
offered (update review H3, step 2). Hash-checking at scan time keeps a
manual copy from silently becoming "the release". Signing the manifest
itself is step 13.

## D-148 Typed refusals: rollouts skip nodes that cannot take an update (amends D-071)

**Context.** A rollout stopped at the first node that refused or was
offline: a helper refusal ("needs systemd") was fatal, and an offline node
failed within seconds (three quick retries). One container node without
systemd blocked the whole fleet, and the refusal was only a string.

**Decision.**
- `CommandResult.error_code` (proto tag 5) carries a
  `forge_proto::messages::RefusalCode`: `unsupported`, `busy` or
  `downgrade`; empty from older agents. Today only `AgentUpdate` sets it:
  the helper (no systemd, no `curl`) and the agent (not Linux, `update`
  not in its policy) say `unsupported`; `busy` and `downgrade` come with
  the swap lock and downgrade refusal (step 5). The helper passes the code
  in its response's `data` (`{"code": …}`); `executor::refusal_code` reads
  it, and `CommandExecutor::execute_coded` keeps it.
- **The rollout waits for offline nodes, not the update jobs.** A batch
  submits a node's `agent.update` once the node is online and skips a node
  still offline after `[update] agent_batch_timeout_secs` (default 1800 s)
  without a job. One worker slot for the whole rollout: waiting inside
  each child held a slot per offline node and could starve the engine
  (security review). Nothing waits inside `send`; `busy` and a single
  update's offline node are `Retryable` (engine backoff), so every attempt
  is authorized again before a new command is signed.
- A rollout's child (`rollout` in the payload, `deadline_ms` = end of the
  batch) **skips** a node that is not Linux, went offline, does not answer
  by `deadline_ms`, or refuses as `unsupported` or `downgrade`: it
  succeeds with `state.skip_reason`. A single update fails instead.
  `agent.rollout` counts skipped nodes (`state.skipped`, `nodes[].outcome`,
  `skip_reason`, `reason`) and carries on; any other failure still stops it.
  The batch waits the timeout plus 10 minutes of grace.
- A batch whose nodes were all skipped validated nothing; the next batch
  is then the canary. Exposure stays bounded by the batch size.
- Cancel: the rollout's wait stops with it, and a child that has not sent
  its command yet fails ("the rollout was cancelled").

**Why.** Owner decision 3: rollouts skip offline and unsupported nodes and
report them. Only a failure of the new release itself is evidence against
it; a node that cannot be reached or cannot self-update is not. Codes, not
message prefixes, so the policy does not depend on wording.

## D-149 Self-update capability in node facts, update verdicts, grouped Update agents dialog

**Context.** The console offered "Update agents" for every outdated online
Linux node, and the operator learned only from failures which nodes could
not self-update (no systemd, no `curl`, read-only `/usr/local/bin`), and
never that an in-container update is lost when the container is recreated.

**Decision.**
- The helper's read-only `Capabilities` answer adds `self_update`
  (`forge_proto::facts::SelfUpdateCapability {supervisor, ready, reasons,
  warnings, helper_version}`; `UpdateIssue`, `Supervisor` with
  `#[serde(other)] Unknown`). The helper probes as root
  (`self_update::probe_host` → pure `assess`): systemd, `curl`,
  `/usr/local/bin` writable and not read-only, free space ≥ 2 × the
  binaries, and an overlay root inside a container (`ephemeral_container`,
  a warning per owner decision 4, not a blocker). The agent asks the helper
  once per fact collection and reports it with `container_exec`; the
  server keeps the `self_update` fact.
- The type lives in forge-proto (part of the agent wire contract), which
  gains a `serde` dependency.
- `agent_update_policy::update_verdict` (pure) → `will_update`,
  `needs_reinstall` (+ reasons, incl. `not_linux`, `no_agent_build`),
  `offline`, `ahead`, `current`, `unknown` (no capability reported: older
  agents, still eligible). `AgentUpdateOverview` (one node list and one
  facts read; on `Services`) feeds `GET /agents/versions` (now typed
  structs), "every outdated node" for rollouts, single updates, the
  follow-up after a server update and automatic updates.
- Facts can be stale, so the verdict only selects and informs: a node named
  explicitly is still sent the update, and the update job relies on the
  node's own refusal (D-148).
- The **Update agents** dialog groups **Will update** (ticked; `unknown`
  marked "not checked", containers warned "until recreated"), **Needs
  reinstall** (reasons, link to the installer guide) and **Offline
  (skipped)**. The UI's types and query live in `src/ui/src/api/agentUpdates.ts`;
  a missing verdict (older server) is derived, never shown empty.

**Why.** Owner decisions 3 and 4 and the review's capability design: the
operator sees before a rollout which nodes it reaches and what to do about
the rest.

## D-150 Crash-safe agent swap: lock, intent marker, fsync, hash idempotency, downgrade refusal (amends D-053, D-054)

**Context.** The helper dry-ran only the new agent, swapped by copy and
rename without fsync, shared one staging folder between requests, decided
"already done" by its own version (a new helper over an old agent answered
"already running"), and a crash between the two renames left a new helper,
an old agent and no marker. The agent confirmed after the first heartbeat,
whichever helper answered, and the control plane's `await` looked only at
the agent's version.

**Decision.**
- **Seams.** `self_update::Layout` (binary paths, state and agent folders)
  and the `UpdateHost` trait (supervised, download, `--version`, restart,
  restart counter); `SystemHost` on nodes, a fake in tests. `apply_in`,
  `confirm_in`, `rollback_in`, `watchdog_start_in` and `recover_in` take
  them.
- **Lock.** `flock` on `update.lock` in the root-only state folder, across
  helper processes. A second update is refused as `busy` (D-148).
  Confirm, rollback and the start-up recovery take it too, so the agent
  user cannot delete `.prev` copies mid-swap (security review); a
  watchdog rollback that finds it busy tries again on its next tick.
- **Idempotency by hashes.** When the installed binaries already have the
  command's hashes, nothing is downloaded; a pending restart for that
  version is asked for again.
- **Downgrade.** `AgentUpdate.allow_downgrade` (proto tag 6, default
  false); without it the helper refuses a version older than its own
  (`forge_sdk::release::compare_release`) as `downgrade`. The agent now
  depends on forge-sdk (adds `semver` only).
- **Staging per request** (`update-staging-<ms>`), left-overs removed
  under the lock.
- **Dry run of both binaries** (`--version` must print the target), with a
  clean environment, the staging folder as working folder and a 30 s limit.
  The helper gained `--version`.
- **Intent marker** (`update-intent.json`: from, to, per file the path, old
  and new hash) before the first rename. Per binary: `.new` beside it
  (`O_EXCL|O_NOFOLLOW`), fsync, hash again, hard-link the current one to
  `.prev`, rename, fsync the folder. Then the pending marker, then the
  intent goes. `recover_in` (helper start, and every update first):
  all files new → finish (pending marker); otherwise put back each `.prev`
  whose hash is the old one, remove a file the update added, and keep the
  intent if a restore failed. It restarts the services.
- **Confirm** only while no intent exists, and the agent accepts it only
  when the helper confirms the agent's own version.
- **Await** also needs the helper: facts collected after the send must name
  the target (or newer) as `helper_version`; an older one, or
  `helper_ok: false` (the agent's helper did not answer), fails the job.
  Older agents that report neither leave `state.helper = "unverified"`.
  Read per node (`FactsRepository::facts_of`).

**Why.** Update review C3 and H5, step 5: a node must never end with a
helper and agent from different releases, or without a working helper,
and a retry must not need the version string to find out what is
installed.

## D-151 Release pipeline and installers for the first DuckDB release (amends D-037)

**Context.** The `v0.1.0-rc-02` run failed in its signing step: the
workflow read `RELEASE_PUBLIC_KEY` from `update.rs`, where only a re-export
was left after the constant moved to `forge-sdk`, so the comparison got an
empty string (fixed in 997c15b). Since then the analytics worker links
DuckDB's bundled C++ (D-143), whose sources do not compile with the GCC 9.2
of the `cross` 0.2.5 musl images, and the owner tests installs on staging
servers straight from GitHub releases. The installers had no signature
check: users were told to compare checksums by hand.

**Decision.**
- **Linux release builds** of both musl targets (x86_64 too) run in the
  `cross` `main` images (crosstool-NG, GCC 14), pinned by digest in
  `Cross.toml`. CI's static musl job uses the same images, so a DuckDB that
  does not compile fails CI before a release.
- **release.yml**: `contents: read` by default; only the new `publish` job
  writes, and only it sees `FORGE_RELEASES_TOKEN`. The first job checks the
  tag, `master` ancestry (error for stable, warning for `-rc-NN`), the
  workspace version, and that `FORGE_RELEASE_KEY` is the private half of
  `RELEASE_PUBLIC_KEY` (the last 32 bytes of ring's PKCS#8 v2 document), in
  seconds instead of after the builds. `check-release.sh` checks every
  asset and each package's layout and `MANIFEST.json` version before
  signing; the signature is then verified independently with OpenSSL. A
  manual run is a dry run unless **publish** is ticked; the assets are kept
  as an artifact. Publishing creates a draft, uploads, then publishes;
  pre-releases are `--latest=false`. Notes come from
  `scripts/packaging/release-notes/<tag>.md` plus generated install,
  file and verify sections with the real asset names.
- **New release assets `get-forge.sh` and `get-forge.ps1`** (the asset
  contract grows; no existing name changes). They download a release by tag
  or `latest` (or take `--file`), verify `SHA256SUMS.sig` with OpenSSL
  against the pinned key and the package's SHA-256, check the architecture
  and the archive's paths, then run the package's installer.
  `FORGE_RELEASES_URL` (also `file://`, `http://`) and
  `FORGE_RELEASE_PUBLIC_KEY` serve mirrors and lab keys. Windows has no
  built-in Ed25519, so `get-forge.ps1` needs `openssl.exe` or an explicit
  `-SkipSignatureCheck`.
- **install.sh** checks the package before it changes anything (folder
  architecture, files, `dist/SHA256SUMS`, both binaries run, 500 MB free),
  opens the ports `forge.toml` configures (`api_listen`, `agent_listen`,
  `[analytics] worker_agent_listen`) instead of fixed ones, health-checks the
  configured API address, waits for the worker's upload port, labels the
  unit and config for SELinux, ends with NEXT STEPS, and gains
  `--uninstall` / `--purge`. Its copy for later uninstalls is
  `/etc/forge/install.sh` (root, `0700`), not in the prefix, which the
  service user owns. `install.ps1` gains the package check, config-driven
  firewall rules and `-Uninstall` / `-Purge`; analytics stays `off` by
  default on Windows.
- `test-install.sh` signs two packages with a throwaway key, serves them as
  a `file://` mirror and tests refusal of a tampered package, install by
  tag, re-install from a local package, upgrade to latest and uninstall, in
  Debian 12 and Rocky 9 systemd containers. CI runs it on full runs and
  packaging changes.

**Consequences.** The Linux images are a moving `cross` branch, pinned by
digest: updating them is a deliberate change. Windows release assets need
`RUNNER_WINDOWS`; DuckDB has not been built with MSVC yet. The public
repository's `README.md` and `INSTALL.md` are still copied by hand.

## D-152 CI on one shared runner: light verification, heavy releases one at a time (amends D-151)

**Context.** Both self-hosted Linux runners ran on one cloud VM (8 vCPU,
15 GiB RAM, an 8 GiB swap file added as a safety net) that also serves the
owner's mail and other services. The first full CI run with DuckDB built
both musl targets at once beside other jobs: both were killed for lack of
memory (exit 137), and an earlier run lost its runner. The host provider
flags an overloaded server.

**Decision.**
- **Actions does two things.** `ci.yml` verifies pushes and pull requests
  with light jobs only (plan, Rust Linux, PostgreSQL, deny/audit, web), path
  filtered; `full` (manual checkbox, default off; also `master` pushes, PRs
  into `master`, `[full-ci]`) only removes the path filter. `release.yml`
  does the heavy work for tags and the dry run: both musl builds, packages,
  signing, publishing and the website image (moved from `website.yml`,
  which is now manual only). The installer test, the fleet benchmark, the
  nightly full run, the weekly benchmark and the musl builds on pushes are
  gone from Actions; `test-install.sh` (now also against a dry run's
  `release-assets`, `FORGE_TEST_ASSETS`) and the benchmarks are local tools
  in the release checklist.
- **Budget: about 4 vCPU and 7 GiB.** Light jobs: `-j 2`, `nice -n 10`,
  `ionice -c3`, two lanes per run. Heavy jobs: `-j 2` (the workflow refuses
  more than 3), the `cross` container capped with
  `--memory=7g --memory-swap=7g --cpus=3` so a runaway link fails the job,
  never the host, and never spills into swap.
- **One host lock instead of `concurrency` for heavy work.**
  `scripts/ci/build.sh light|heavy` takes a shared or exclusive `flock` on
  a file both runners see. Heavy waits for every compile on the server to
  finish and blocks new ones; it works across workflows and runs. A job
  `concurrency` group was rejected: a new pending job in a group cancels
  the pending one, which could cancel a release build waiting its turn.
  Within a release run, `max-parallel: 1` and `needs` order the heavy jobs.
- **Latest wins**: `ci.yml` cancels the older run of a branch or PR;
  release dry runs of the same tag replace each other; tagged releases
  never cancel.
- **Build cache**: release builds keep `_forge-target/<target>` per runner
  outside the checkout; `runner-hygiene.sh` deletes it past 20 GB or when
  the disk is short, and prints the host's CPUs, memory and disk at the
  start of every job.

**Consequences.** CI no longer proves the musl build or the installers on
every push: a release dry run and the local installer test before tagging
do. Heavy jobs can wait a long time for the lock (timeouts include the
wait). Whether fat LTO of `forge-analytics-worker` fits the 7 GiB cap is
measured separately; any LTO change for shipping builds is an owner
decision.

## D-153 Open source under Apache-2.0, one public repository (supersedes D-131's licensing and release split, and D-152's runner setup)

**Decision.** Forge is open source under the Apache License 2.0. The root
`LICENSE` is the single copy: packages ship it as `LICENSE.txt` and the
website's license page renders it. The Orbyn Forge Free Use License is retired.
Source, releases, install scripts and issues all live in the public
`orbyntech/forge` repository. The release workflow publishes there with
`GITHUB_TOKEN`, and the updater's default feed is
`https://api.github.com/repos/orbyntech/forge/releases/latest`. CI and release
builds run on GitHub-hosted runners only. Windows packages and the macOS CLI
are always built. Contributions arrive as pull requests from forks into
`develop`. The public repository starts from a fresh history.

**Why.** Forge is no longer developed as a commercial product. One public
repository with a standard license is the simplest thing for users and
contributors. Public repositories get hosted runners for free, so the
self-hosted runner setup and its lock and cleanup scripts are no longer
needed.

**Consequences.**
- Installs that still point at `orbyntech/forge-releases` need one last
  release there that moves them to the new feed. After that release, the old
  repository is archived.
- Lab machines in docs and scripts are named `lab-a` … `lab-z`.
- Example addresses use `192.0.2.x`.

## D-154 Cluster modes: Default and Docker Swarm (supersedes D-033)

**Decision.** Every cluster has a mode, `default` or `swarm`. You choose it at
creation, and it cannot be changed afterwards. It is stored in
`clusters.mode`, added by migration `0002`. Existing clusters migrate as
`default` and behave as before.

- **Default clusters** keep Forge's own orchestration, for system services and
  containers.
- **Swarm clusters:**
  - Every node runs Docker Engine and joins one Docker Swarm. Forge inits the
    swarm on the first manager and joins the other nodes during onboarding.
  - Swarm schedules containers. Only container services are allowed.
  - Projects, environments, applications and services keep their meaning.
  - Deployments still go through Forge. Its strategies map onto Swarm's
    native update and rollback config where that fits; blue-green and canary
    are built from paired services behind Forge's ingress.

**Why.** D-033 parked Swarm until a workload needed scheduling across nodes.
Container management at the level of Portainer needs that now: replicas,
placement, node drain and rolling updates, all driven by CI. Docker already
does this well, so Forge drives Swarm and does not build a scheduler of its
own.

**Not done yet.** Converting an existing cluster to the other mode.

## D-156 Deployment strategies on Docker Swarm

**Decision.** Releases in a Swarm cluster go through the same deployment record, events, pause, abort and approval as node rollouts. The steps come from a Swarm plan (`forge_catalog::swarm_rollout`):

| Strategy | On Swarm |
| --- | --- |
| Rolling | The service's own update: `batch_size` tasks at a time; start or stop first; Swarm rolls back on failure when `auto_rollback` is set |
| Recreate | The same update with every task at once, stop first |
| Blue-Green | A second Swarm service (slot `a`/`b`) at full size on Forge-allocated ports; once healthy, the edge moves all traffic to it; the old slot is scaled to zero and kept |
| Canary | The new slot grows step by step while the edge gives it the same share of traffic; then it takes all traffic |
| Manual | Refused: Swarm cannot hold an update between batches |

HA is a property, not a strategy: the service's Swarm view scores replicas, spread, health check, restart, rollback on failure and manager quorum.

**Why.** Swarm already does rolling updates well, so Forge uses its update config instead of driving tasks itself. Swarm cannot split traffic between two versions of one service, so Blue-Green and Canary use two services and the weighted routing that node rollouts already use (slot records and the edge), which keeps one model for both cluster modes.

**Trade-offs.** Blue-Green and Canary need a site in front of the service. Slotted and unslotted services do not switch strategies in place. Failure handling moves traffic back to the old slot, or lets Swarm roll the service back.

## D-157 Docker images, volumes and networks through the node's helper

**Decision.** Every node's Docker images, volumes and networks can be managed from Forge through a closed, typed operation set (`forge_catalog::docker_ops::DockerOp`). The node's root helper runs it over the local Engine API. Reads need `server.manage`. Changes need `docker.image.manage`, `docker.volume.manage` or `docker.network.manage`, held by administrators and operators. Pulls, pushes and prunes run as `docker.op` jobs; everything else answers at once. Every change is audited as `docker.<op>`.

**Limits.**
- Volumes are created with the local driver and without options, and networks as plain bridge or overlay. Driver options can bind host paths, so they are never accepted.
- Docker's networks (`bridge`, `host`, `none`, `ingress`, `docker_gwbridge`) and Forge's (`forge-*` or labelled `forge.managed`) cannot be removed, connected or pruned. The node resolves an id to the network's name before it checks.
- Registry credentials never pass through this API, because job payloads are stored. Private registries come with the registry model.

**Why not the Engine API proxy (D-125)?** The proxy needs the node's mutual-TLS endpoint, which is an opt-in that restarts dockerd. The typed set works on every node, and its smaller surface is easier to review. The action names have a `docker.` prefix because `network.manage` is a retired action, and reusing it would quietly grant the new rights to old stored profiles.

## D-158 Container registries

**Decision.** A workspace keeps container registries (`docker_hub`, `ghcr`, `gitlab`, `ecr`, `custom`) with a host, a user name and a secret. The secret is sealed with the envelope under `registry:<id>`; no API answer, log, audit entry or job payload carries it. When Forge pulls, pushes or deploys an image, it matches the image's host (Docker Hub for references without one) to a registry. It then adds Docker's `X-Registry-Auth` to the transient signed command only: `docker.op` steps, Swarm service applies (`--with-registry-auth`) and Swarm rollouts.

Managing registries needs `registry.manage`: administrators, with MFA. Pushes go only to registries added in Forge, so an operator cannot send an image to an arbitrary registry.

**Not yet.** ECR tokens expire after 12 hours and are not refreshed automatically; update the secret, or use a custom registry with a long-lived credential. Compose stacks do not use registries yet.

## D-159 Deploy webhooks

**Decision.** A deploy webhook is a secret URL, `POST /hooks/<token>`, that CI calls to deploy a service (on Default or Swarm clusters) or a compose stack. The token (`whk_…`, 256 bits) is stored as its SHA-256. An optional signature adds a second factor:
- `github` uses `X-Hub-Signature-256`, the HMAC of the body;
- `gitlab` uses `X-Gitlab-Token`;
- `forge` uses `X-Forge-Signature`, the HMAC of `<timestamp>.<body>`, with `X-Forge-Timestamp` within five minutes to stop replays.

The secret is sealed. The token and the secret are shown once and can be rotated.

A delivery runs as the webhook's creator, through the same deploy paths and checks as the console. Permissions are re-checked on every delivery; the delivery never counts as MFA, so MFA-guarded work (privileged binds) is refused. It is audited as `webhook.deliver`, with the webhook in `via`.

A delivery may pick the image tag (`?tag=` or a JSON `"tag"`) only when the webhook has a tag pattern such as `v*`. Deliveries are rate-limited per source address (before any lookup) and per webhook. The newest 100 deliveries per webhook are kept. `GET /hooks/<token>/jobs/<id>` reports only jobs that webhook started.

**Why.** CI needs one call that deploys without holding a user's token. A URL credential that can only deploy one target, is checked again against its creator's current rights, and leaves a record of every call is narrower than a service-account token. Signatures, especially the timestamped one, stop a leaked URL being replayed from a log.

## D-160 Compose app templates

**Decision.** Forge ships a small set of compose app templates: Nginx, PostgreSQL, Redis, MariaDB, WordPress and Uptime Kuma. Each has pinned images (never `latest`), health checks and named volumes. A template's `${VAR}` placeholders are filled from the stack's `.env`, so Docker Compose does the substitution and Forge renders nothing into YAML. Secret variables left empty get a random 192-bit value and are sealed like any stack `.env`. A stack made from a template goes through the normal stack checks: policy, MFA for privileged settings, and audit.

**Not done.** Git-backed stacks, where a stack follows a branch and redeploys on push. They need a git fetcher on the control plane and deploy keys, which means a new dependency and a new trust boundary, so they are on the roadmap. Until then, a deploy webhook (D-159) redeploys a stack from CI.

## D-161 Address lists and API-wide rate limits

**Decision.**
- **Address lists, `[security.access]`:** `deny` is refused everywhere. A non-empty `allow` limits the console, API, MCP and OAuth. `webhooks_allow` and `nodes_allow` apply instead of `allow` to `/hooks/*` and to the node installers and agent downloads, so CI and new nodes keep working while the console is closed to the internet. The server refuses to start with an `allow` list that excludes loopback or a trusted proxy.
- **Rate limit, `[security.rate_limit]`:** a sliding minute per client address, 1200 by default, with an IPv6 `/64` keyed as one address.
- **One limiter:** a single bounded limiter (`rate_limit::Windows`) is shared by the API limit, the webhook limits, replay protection and enrollment attempts. When it is full of active keys it refuses newcomers rather than resetting, because a reset would lift every limit during a flood.
- **Specific limits:** agent enrollment is limited to 20 attempts per minute per address. Agent downloads are streamed, with at most 16 at once.

**Why.** Before this change, only sign-in and a few endpoints had limits, and the control plane could not be closed to unknown networks without a firewall in front. The lists use the same client-address logic as the sign-in throttle, so a reverse proxy is handled the same way everywhere.

## D-162 New release signing key for the public repository

**Decision.** The first public release candidate, `v0.0.1-rc-01`, is signed with a new Ed25519 release key. Its public half, `6yf7l7jBEw6u446vqWAgt7P7bqEuaY5tpbYnWPHBmbg`, replaces the earlier key in `RELEASE_PUBLIC_KEY`, `get-forge.sh`, `get-forge.ps1`, the installation guide and `.gitleaks.toml`. The private half is the `FORGE_RELEASE_KEY` Actions secret of `orbyntech/forge`. It was generated on the maintainer's machine with `forge-server release keygen` and never left it except into that secret.

**Why.** The public repository had never published a release, so no install trusted the old key. The key could therefore be swapped directly, without the rotation release a key change otherwise needs (see development/releases in the docs). The version also restarts at 0.0.1 for the public line; the earlier `v0.1.0-rc-*` tags from the private repository were dropped.

## D-163 The website moves to its own repository

**Decision.** The website (`website/`) moves to `github.com/orbyntech/forge-website` and is hosted separately. The documentation stays in this repository (`docs/`). The website image clones this repository at a chosen ref, builds `docs/` and serves it at `/docs/`, so `forge.orbyntech.com/docs/` links keep working. The docs keep their own copy of the brand tokens (`docs/src/styles/tokens.css`). This repository's CI no longer builds the website, and releases no longer push its image.

**Why.** The website changes on a different rhythm from Forge and needs no Rust CI, release signing or code review by Forge maintainers. Keeping the docs here keeps them next to the code they describe, where a change and its docs land in one pull request.

## D-164 The repositories belong to the orbyntech organization

**Decision.** `forge` and `forge-website` were transferred from the maintainer's personal account to the `orbyntech` GitHub organization: `github.com/orbyntech/forge` and `github.com/orbyntech/forge-website`. The release feed compiled into Forge (`DEFAULT_FEED`), the install scripts, the docs and the website image (`ghcr.io/orbyntech/forge-website`) use the organization's URLs. The rulesets, the `FORGE_RELEASE_KEY` secret and the release key (D-162) moved with the repository unchanged.

**Why.** The project is published by Orbyn Technologies, so its source, releases and issues belong to the organization. GitHub's transfer keeps history, pull requests and redirects from the old URLs, and no release had been published yet, so no install points at the old feed.

## D-165 One branch: master

**Decision.** `develop` is removed. `master` is the default and only long-lived branch: work happens on short branches cut from `master` and lands through squash-merged pull requests into `master`. CI runs on every pull request into `master` and every push to it. A tag on `master` runs the release pipeline. The branch ruleset now protects `master` only: pull request and green CI required, no force pushes or deletion, and only organization admins may merge, tag or publish releases.

**Why.** With one maintainer, the `develop` → `master` → `develop` round trip added two pull requests and two CI runs per release without catching anything a pull request into `master` does not.

## D-166 Faster release builds: native Alpine builds and a prebuilt DuckDB

**Decision.**
- Both Linux release targets build natively in one pinned Alpine image (`rust:<version>-alpine3.22@sha256:…`, set in `scripts/ci/alpine.sh`): `x86_64-unknown-linux-musl` on `ubuntu-latest`, `aarch64-unknown-linux-musl` on `ubuntu-24.04-arm`. Alpine's musl-native GCC and libstdc++ compile DuckDB's bundled C++. `release.yml` no longer uses `cross`; `Cross.toml` stays for local builds.
- `.github/workflows/duckdb-prebuilt.yml` compiles the release dependencies (DuckDB included) in the same image, at the same in-container paths and with the same first cargo command, and publishes them to GHCR as `ghcr.io/orbyntech/forge-duckdb:<libduckdb-sys version>-<rustc version>-<target>`. The release build restores that artifact first, so Cargo finds libduckdb-sys fresh and skips the C++ build. A missing or mismatched artifact only makes the release slower.
- `[profile.release]` keeps `lto = "fat"` and `codegen-units = 1`.
- The `forge` CLI keeps its own cargo invocation: building it together with the server and agent would unify features and pull its keyring dependencies into those binaries.

**Why.** A from-scratch release build took about 33 minutes per Linux target. The build logs showed that compiling DuckDB's 2,129 C++ files (`libduckdb-sys`, feature `bundled`) took 20 to 23 minutes of it, already in parallel. Thin LTO was tried and bought no build time, since DuckDB, not LTO, was the critical path, so the release profile went back to fat LTO. Tag builds cannot reuse caches: GitHub "cannot restore caches created for different tag names". A versioned artifact keyed on what DuckDB's output depends on (its crate version, the compiler, the target) works across tags. Cross-compiling DuckDB's bundled C++ is "best-effort" upstream; a native build avoids it.

**Consequences.** Bumping `libduckdb-sys` or the pinned image makes the next release compile DuckDB until `duckdb-prebuilt.yml` has run on `master` (it runs automatically when `Cargo.lock` or `scripts/ci/alpine.sh` changes). Crate sources in the build container get an old fixed mtime so build scripts do not rerun because of a fresh extraction; their content is pinned by `Cargo.lock` checksums.

**Reverting.** Delete the restore step in `release.yml`, or point the Linux matrix entries back at `cross` on `ubuntu-latest`.

## D-167 Any-address control plane

**Context.** Forge needed one fixed console URL (`public_url`), asked for at
install. Agents checked the server certificate's host name against
`agent_hostnames`, so a node could only use the names baked in at install, and
a server on several networks, behind NAT or renamed later needed config edits
and re-enrollment. The browser `Origin` checks compared against `public_url`
and the runtime public address only (D-120).

**Decision.**
- **No fixed address.** `public_url` is optional; installers no longer ask
  for it. The console answers on every name and IP. A `RequestOrigin`
  extractor takes the request's scheme and host from `Host`, or from the last
  `X-Forwarded-Host`/`X-Forwarded-Proto` of a peer in `trusted_proxies`, and
  validates it strictly (else `400`).
- **Allowed hosts.** A runtime setting `server.allowed_hosts` (default `*`;
  `*`, exact host or IP, `*.domain`) is enforced as middleware: other hosts
  get `421` with no body. Edited in Settings → Server, the API
  (`/server/allowed-hosts`) and `forge-server allowed-hosts`; audited.
- **Same-origin checks.** WebSocket and `/mcp` `Origin` must equal the
  request's own origin on an allowed host, or the pinned address. This
  supersedes the fixed-origin rule of D-120.
- **Links.** Request-time links (Add node commands, install scripts, device
  and approval links, OpenAPI) use the request's address. The agent URL is
  `https://<request host>:<agent port>`, so each node enrolls on the address
  it reached. Background links (alerts, webhooks, mail host, OIDC, OAuth
  issuer) use the pinned address (`server.public_address`, else
  `public_url`), else the address a platform administrator last signed in on
  (`server.last_admin_address`). OIDC refuses to start without a pinned
  address.
- **Passkeys** use the request's host and origin as relying party.
- **Pinned server key.** The agent and analytics upload listeners present
  Forge CA certificates for one long-lived control-plane key, sealed with the
  envelope key in the settings table (`agent_tls.keys`) and renewed in place
  with the same key. Agents accept the server only if the leaf's
  SubjectPublicKeyInfo SHA-256 is in their pin set (at most 8; root-owned
  `/var/lib/forge-agent-pins/pins.toml` on Linux), the certificate is valid
  now and has ServerAuth. No host-name check. The enrollment token carries
  `server_key_sha256`; heartbeat replies announce current and next keys,
  signed with the command key and applied by the root helper.
- **Rotation.** `[agent_tls] rotate_days` (365; 0 = manual) and `grace_days`
  (30): the server switches to the next key only after the grace period and
  once every online pinning agent acknowledged it, or at once with
  `forge-server agent-key rotate --now`. The previous key is kept sealed for
  one rotation. Manual recovery: `forge-server agent-key show|list|rotate`,
  `forge-agent pins list|add|remove|fetch`. A mismatch names both
  fingerprints and the commands to run.
- **Analytics port.** Agents get the worker's port and dial it on the host of
  their own `server_url`; the full `analytics_url` is still sent.

**Why.** A fixed address is the most common install question to get wrong,
and it breaks for multi-homed servers, NAT and later domains. Pinning a key
instead of a name keeps the agent link as strict as before (stricter: another
certificate from the same CA is refused) while making the address irrelevant.
Request-derived links match what the operator sees; keeping them out of
emails, webhooks and OIDC prevents Host-header link poisoning.

**Security notes.** Forwarded headers count only from `trusted_proxies`, and
`0.0.0.0/0` and `::/0` are refused there. Only the last forwarded value is
used. `*.domain` matches at a dot boundary and needs two labels. HSTS is sent
only on HTTPS responses. Key material never leaves the server unsealed; only
fingerprints appear in output, the API and audit (`agent_key.create`,
`agent_key.rotate`, `agent_key.rotate_request`, `server.allowed_hosts.*`).
Pins are root-owned on Linux; only signed announcements and root
`forge-agent pins` change them, and `pins fetch` needs confirmation or
`--yes`. `/install.sh` is still plain HTTP behind a token: an existing risk;
the docs recommend HTTPS for untrusted networks. Listening on `0.0.0.0` is
narrowed by the cloud firewall, the host firewall and `[security.access]`.

**Compatibility.** Installs with `public_url` keep it as the pinned address;
allowed hosts default to `*`, so nothing is refused after an update. A
`public_url` of `0.0.0.0` or with a path now stops the server at start.
Existing agents keep CA plus host-name checks until they receive a pin over
their existing link; the listener certificate keeps `agent_hostnames`.
Older agents keep using `analytics_url`. Passkeys stay bound to the name they
were registered on.

**Security review follow-ups.**
- The last administrator address is recorded only when the request host is
  loopback, an address or the host name of the server, a configured name or
  a match in an explicit (not `*`) allowed-hosts list; changes are audited
  (`server.last_admin_address.update`). The MCP OAuth issuer and resource
  use only the pinned address; without one the connector endpoints answer
  `409 no_public_address`.
- With allowed hosts `*` the `Origin` checks guarantee same-origin only, not
  DNS-rebinding protection; the console recommends narrowing the list.
- Key announcements carry a sequence (bumped on every change of the key
  set) and a revoked list; nodes refuse older or conflicting ones, unpin
  revoked keys at once and never re-add them from an announcement. They are
  valid for 5 minutes. `agent-key rotate --now --revoke` (`agent_key.revoke`).
- The analytics worker listener has its own key pair (current and next,
  rotated with the agent key), announced as `analytics` pins; the worker no
  longer holds the agent hub key.
- An existing but unusable pin file stops the agent from connecting.
- Acknowledgements keep only this server's fingerprints, are rate-limited per
  agent and pruned for deleted nodes. The installers are `no-store`.

**Amendment (enrollment as the agent user).** The installer enrolls as
`forge-agent`, which cannot write the root-owned pin file. Enrollment then
stages its set in `pins.enrolled.toml` in the state directory, and
`forge-agent-helper pin` (root, right after enrollment, the same moment it
trusts `identity.json` per D-068) adopts it only when no pin file exists yet,
after checking it like the pin file. The agent user never changes an existing
set. The agent refuses to start while a staged set waits and the pin file is
missing, rather than start unpinned. The release workflow stamps each
release's tag into its `get-forge.sh` and `get-forge.ps1` as the default
version, so a copy downloaded from a pre-release installs that pre-release.

## D-168 Deploy webhooks: signed tags only, durable replay protection

**Context.** A security review of deploy webhooks (D-159) found that the
`?tag=` query was honoured on signed webhooks although no signature covers
it, so a captured delivery could be replayed with another tag; that replay
markers lived in memory and were lost on restart, and a GitHub signature has
no timestamp, so any captured delivery replayed after the 600-second window;
that the GitHub marker preferred the unsigned `X-GitHub-Delivery`, which an
attacker can change; and that a GitLab delivery without
`X-Gitlab-Event-UUID` skipped the replay check altogether.

**Decision.**
- A webhook with a signature takes the tag only from the JSON body. `?tag=`
  is refused (`422`). Unsigned webhooks cannot change the tag at all (D-159).
- Replay markers are stored in `webhook_replay_markers` (migration 0003),
  keyed `(webhook_id, marker)`, claimed with `INSERT … ON CONFLICT DO
  NOTHING` through `WebhookRepository::claim_replay_marker`, and pruned after
  30 days by the `Clock` on each claim. The marker is the SHA-256 of the
  GitHub body signature, the Forge signature or the GitLab event id. A store
  failure refuses the delivery (`503`, fail closed).
- GitLab deliveries must carry `X-Gitlab-Event-UUID` (`400` otherwise).
- A webhook tag change is compensated: the new image is saved, and when the
  deploy fails the previous spec is put back.
- Prune dialogs in the console state that a prune is host-wide; prune
  semantics are unchanged.

**Consequences.** GitHub's **Redeliver** of an identical payload is refused.
A GitHub delivery can be replayed after 30 days; rotating the secret ends
that. GitLab mode remains the weakest (a static token); the docs recommend
Forge or GitHub signatures. The compensating restore can race a concurrent
console edit; that is recorded as CODE-02 in `plans/code-quality.md`.

## D-169 Ingress endpoints: one list, typed features, read-only main config

**Context.** Endpoints were split between cluster ingress pages and
hand-picked edges, and anything beyond proxying (redirects, static files,
caching, basic auth, per-path routing) had to go into the free custom block,
which runs as root inside Nginx and is allowlisted for that reason. Operators
asked for a full site editor in the style of hosting panels such as aaPanel,
without giving up the allowlist or adding a raw-file mode that would let a
config edit bypass it.

**Decision.**
- One endpoint list with an explicit placement on every endpoint:
  `{kind: nodes, server_ids}` (nodes with Nginx) or `{kind: cluster,
  cluster_id}` (the cluster's ingress profile decides the edges). The older
  `edges` and `cluster_id` fields are still read when `placement` is absent,
  so stored endpoints need no migration.
- Typed features on `SiteSpec` instead of a raw-file mode: the default rule's
  `kind` (proxy, balancer, redirect, static), path `rules`, `redirects`,
  `cache`, `gzip`, `access` (allow/deny, basic auth, hotlink), ports, `hsts`
  and `rate_burst`. Each part validates its own input, so the renderer only
  writes checked values. The aaPanel model: the main configuration is
  generated, shown read-only and rewritten on every apply; the custom file
  stays the only free text, included in the `server` blocks, allowlisted,
  `nginx -t` tested and rolled back, and gated by `server.command` (MFA) on
  every edge. New fields are skipped when at their defaults, so endpoints
  that use none of them reach older agents unchanged.
- Basic auth is a form feature. Forge hashes passwords with SHA-512 crypt on
  save, stores only the hash and never returns it (API responses drop
  `hash`; previews use a placeholder). A user sent without a password keeps
  the stored hash. Agents write `/etc/nginx/forge/htpasswd/<name>` mode 0640
  for Nginx's group.
- Static roots must sit inside an approved directory or service file root
  (never a node or container root) on every edge, and the caller needs
  `file.read` there. The endpoint flow marks its agent commands with
  `from_endpoint`; a `site` command serving a static directory without that
  marker needs `server.command`, so `POST /servers/{id}/commands` cannot
  publish `/etc`.
- Per-endpoint logs at `/var/log/nginx/forge/<name>.{access,error}.log` and
  a log viewer (`GET /sites/{id}/logs`, last 1000 lines / 512 KiB,
  `server.manage` on that edge). `POST /sites/{id}/cache/clear` empties
  `/var/cache/nginx/forge/<name>` (`proxy.manage`).
- No new Cedar actions: existing `workspace.manage`, `proxy.manage`,
  `server.manage`, `server.command`, `file.read` and `certificate.use` cover
  every operation.

**Consequences.** Older agents refuse the new fields (`deny_unknown_fields`),
so edges must be updated before an endpoint uses them; the upgrade note says
so. Logs move from `/var/log/nginx/access.log` to per-endpoint files on the
next apply. Moving an endpoint between placements does not clean up its old
edges; the console warns to remove it first. Features not modelled still go
through the allowlisted custom file.

## D-170 Object stores: Docker settings, discovery and adoption of any RustFS container, keys kept in RustFS IAM

**Context.** Docker stores ran with fixed container settings, so operators
could not set limits, a restart policy, named volumes or engine variables, and
could not change release, ports or region after install. Only the App Store
`rustfs` container could be adopted, although operators often run RustFS
containers they created themselves. Access keys were limited to one whole
bucket, and Forge's view of them could disagree with RustFS when someone
changed keys in the RustFS console.

**Decision.**
- Docker stores take container settings (`RustfsSettings`: image, name,
  restart policy, network, memory and CPU limits, extra variables, a bind
  path or named volume per drive), stored in `object_store_settings`
  (migration 0004). Extra variables come from an allowlist; credentials,
  listen addresses, volumes and the console switch can never be set.
  Distributed stores always use the host network. Stores take an S3 region.
- `PUT /object-stores/{id}/settings` changes release, ports, region and
  container settings. Docker: pull first, keep the old container stopped as
  `<name>-forge-previous`, start the new one, wait up to 90 s for health,
  and on failure restore the old container and env file; the store record is
  restored from the job's `previous` (explicit compensation). Native stores
  re-run the install in place. Container name, network and volumes are fixed.
- Discovery (`GET /object-stores/discover`) lists RustFS containers (image
  `rustfs/rustfs` or label `org.orbyn.forge.objstore`) and Docker presence
  per node, on nodes the caller holds `server.manage` on. It reads no
  environment values. Any such container can be adopted; the root credential
  is read on the node, reported once on a line the agent strips, and sealed
  in the vault. A container without a credential uses `rustfsadmin` and the
  job warns. Adopting needs `server.manage` on the node too, and
  `endpoint_host` must be the node's own address, since root-signed admin
  calls go there. Adopted variables that fail validation are dropped.
  Adopted containers are not editable and never reinstalled; removing the
  store only forgets the container, which keeps running with its data. Forge
  never deletes named volumes.
- After security review: the env allowlist excludes `RUSTFS_OBS_ENDPOINT`
  (telemetry egress) and `RUSTFS_TLS_PATH` (arbitrary paths). A custom image
  or a volume driver/options also needs `server.manage` on every member.
  New bind host paths follow the data-path rules. A failed volume creation
  during reconfigure rolls back.
- RustFS IAM is the source of truth for access keys. A key holds up to 32
  grants (bucket or prefix, `read`/`readwrite`) rendered as its inline
  policy; a prefix grant is object actions on `bucket/prefix*` plus
  `s3:ListBucket` with `StringLike s3:prefix "prefix*"`. `object_store_keys`
  is a cache and audit trail. Drift is reported per key (`in_sync`,
  `policy_changed`, `missing_in_rustfs`, `not_in_forge`, `unknown`);
  reconcile accepts RustFS's state. While RustFS is unreachable the cached list
  is read-only and writes return `503 store_unreachable`.
- No new Cedar actions. `storage.manage` (MFA) covers settings, adoption and
  IAM; `server.manage` is reused for discovery (it runs `docker` on the node),
  container adoption, custom images and volume drivers.

**Consequences.** Operators can tune and upgrade Docker stores in place with
an automatic rollback, and bring existing RustFS containers under Forge
without moving data. Changes made directly in RustFS are visible and are never
silently overwritten, but Forge cannot change keys while RustFS is down.
Bucket grants for people (D-108) are unchanged: they stay Forge-level and never
issue S3 credentials. Named volumes left after an uninstall must be removed by
hand.

## D-171 Docker Swarm end to end: stacks, secrets, shared network, mesh addresses, conversion

**Decision.** Swarm clusters get the rest of what Swarm offers, driven through the same signed `SwarmOp` set (D-154):

- **Stacks.** A compose stack whose environment is in a Swarm cluster is deployed as one `docker stack deploy` on a manager (`swarm.stack` job), not per node. The stack keeps its row, revisions and sealed `.env`; `POST /stacks/{id}/deploy` and `/down` route there. The compose policy is the same `forge_catalog::stacks::check`, plus Swarm rules (`forge_catalog::swarm_stack`): `secrets:`/`configs:` must be `external`, relative binds and `network_mode` are refused, `depends_on` and `container_name` are dropped with a note, host binds must be inside a root approved on every node.
- **Shared network.** Every swarm can have `forge-shared`, an attachable, encrypted overlay. Stack services that publish `ports:` (or set `x-forge.shared: true`) join it, so other stacks reach them by name; the rest stay on the stack's own encrypted overlay.
- **Secrets and configs.** Docker is the record; Forge labels what it creates. Values arrive in the request (or from a vault item) and go only into the signed command; they are never stored, logged, audited or returned. Swarm objects are immutable, so a rotation creates `<name>.v<N+1>`; stacks and services always mount the newest version on their next deploy, and a prune removes old unused versions. Single Swarm services mount them too (`swarm.secrets`, `swarm.configs`), and `swarm.secret_env: files` moves secret variables into Swarm secrets (`/run/secrets/<KEY>` plus `<KEY>_FILE`), so they leave the service spec. Managing objects is `swarm.manage` (administrators, MFA).
- **Mesh addresses.** The join form and `forge swarm addresses` suggest the node's addresses with the Forge fabric address first; the agent sends `DataPathAddr` equal to an IP advertise address, so overlay traffic follows it across NAT and sites.
- **Volumes.** `volume_create` accepts a typed NFS or CIFS share (checked fields, no free-form driver options) and lists show which containers use each volume.
- **Visibility.** Stack status lists every task with history; per-task CPU and memory come from one `TaskStats` command per node that runs the viewed tasks, cached for ten seconds.
- **Ingress.** A new target kind `swarm` (cluster and published port) resolves to every joined swarm node (the routing mesh); `app` targets of Swarm services without slots do the same.
- **Conversion.** `POST /clusters/{id}/convert` switches Default → Swarm when every node runs Linux, no service is pinned to nodes and no stack runs per node. Swarm → Default needs every node to have left and no Swarm service left; Forge does not move running Swarm tasks back onto nodes, because that needs a placement decision only the operator can make.

**Why.** Users run whole applications on Swarm, not single services. Reusing the compose policy and the stack model keeps one review surface; letting Docker hold secrets avoids a second secret store with its own rotation story.

**Trade-offs.** `docker stack deploy` needs the Docker CLI on managers (it ships with Docker Engine). A CIFS password is kept by Docker in the volume's options on that node. Hash-derived versions of secret variables can leave old versions behind until a prune.

## D-172 Container addresses come from the node's instance block via a routed Docker bridge; no per-container WireGuard; Docker 28 minimum

**Decision.** On every Linux member of a Default cluster whose fabric is in sync, Forge creates one Docker bridge network `forge-<iface>` whose subnet is that node's instance block (already in every peer's `allowed_ips`), with MTU 1420, bridge device `fg<first 8 chars of iface>` and `gateway_mode_ipv4=routed`; `net.ipv4.ip_forward` is enabled and persisted in `/etc/sysctl.d/90-forge-fabric.conf`. Every compose service joins it under the reserved key `forge-mesh` unless it sets `network_mode` or `x-forge.mesh: false` (service or top level). The helper injects the network after its policy check; an external network named like a cluster network is allowed only when it is the node's own.

- **Gate.** The server sets `StackPart.mesh_network` only when the agent reports the capability fact `"mesh_network": true` (missing or unreadable = older agent, plus an Info finding). There is no version constant: agent build versions are not comparable.
- **Docker 28 minimum.** Routed gateway mode needs Docker 28. An older or missing Docker is an `unsupported` refusal: the node's WireGuard part stays in sync, the network status is `unsupported`, and it is retried only when the `docker_version` fact changes, on **Re-sync now** and on the 10-minute repair pass. Swarm clusters and Windows nodes get no container network.
- **Other network failures.** Any other failure to create the network (an overlapping Docker network, a failed sysctl, a lost node) sets the network status to `failed` with the error and leaves the WireGuard state as it is, so the node keeps its mesh address, its stacks keep binding ports there and other nodes keep reaching it. It is retried on **Re-sync now** and the 10-minute repair pass.
- **Native containers.** A runtime `container` service is connected to `forge-<iface>` as a second network after create, under the same gate, unless its spec sets `"mesh": false`; the default bridge and published ports are unchanged.
- **Subnet change.** The helper refuses to move a network with containers attached; `PUT /clusters/{id}/fabric` returns `restarts_containers` and `redeploy_nodes`.
- **Conversion and removal.** Default → Swarm removes the network on every Linux member and is refused with `409` naming node and containers when any are still attached (only something outside Forge can be); offline nodes keep their empty network. The fabric stays for Swarm's data path. Swarm → Default needs nothing: the next reconcile recreates it. Fabric removal removes the network first.

**Why.** The mesh already routes each node's block, so a routed bridge gives every container a cluster-reachable address without touching WireGuard peers, NAT or per-container keys, and Docker allocates the addresses.

**Trade-offs.** Docker 28 is required for the feature. There is no filtering between environments on this network. Moving to a new subnet needs a redeploy of the stacks on affected nodes.

## D-173 forge-dns: a standalone cluster DNS binary on hickory

**Decision.** Cluster names (D-175) are served by a new binary, `forge-dns` (`src/apps/forge-dns`), built on `hickory-server` and `hickory-proto` 0.26 with default features off: plain UDP and TCP only, no DoT, DoH, DoQ, DNSSEC or resolver features, so no TLS stack and no OpenSSL enters the tree. It reads `names.json` (`forge_catalog::names::Records`), polls the file's mtime and size every 2 s instead of using `notify`, swaps the answer map in whole and keeps the old map on a parse failure. Names outside the zones are forwarded to the upstreams in order (1 s each) with no cache. It runs as its own systemd unit `forge-dns.service` (user `forge-dns`, only `CAP_NET_BIND_SERVICE`) on nodes and, when `[dns] listen` is set, on the server host.

- **Licence.** hickory-server, hickory-proto and hickory-net are `MIT OR Apache-2.0`; their remaining new transitive crates are permissive and `cargo deny check` passes.
- **Until a valid file is read** every query is REFUSED: the zones are not known yet, and forwarding would leak cluster names upstream.

**Why.** A separate process keeps a resolver bug or restart away from the agent and the server, needs only the low-port capability, and can be restarted when its listen address changes without touching the agent. hickory is the maintained pure-Rust DNS server; writing a parser ourselves would mean owning truncation, EDNS and TCP framing. PowerDNS or CoreDNS would be one more foreign package per node for a few hundred records.

**Trade-offs.** One more binary in the release and on every node (a few MB). No caching: every forwarded query costs a round trip to the upstream. The 2 s poll means a name follows a change within about 2 s plus the TTL.

## D-174 forge.stack.yaml and the stack.rollout job

**Decision.** A compose stack can be described by a `forge/v1` `Stack` document (`forge_catalog::stack_doc::StackDoc`, D-174): the stack's name and environment, its compose file (`file` or `inline`) and `.env` file, placement (`nodes` or `cluster`), `network.mesh` and the rollout (`order`, `healthTimeout` 10 s–1 h), with per-environment `overrides` limited to `placement`, `network`, `rollout` and `compose.envFile`. Unknown fields anywhere are refused with their dotted path. `forge stacks validate / plan / apply` read the document and the files it names and send their text; the server never reads files. `POST /stacks/plan` has no side effects (audited as `stack.plan`); `POST /stacks/apply` reuses the stack create and update paths (a revision noted `apply <sha256>[:12]`) and starts a `stack.rollout` job.

- **Into the compose file.** `placement.cluster` and `network.mesh: false` become a top-level `x-forge` block (`cluster`, `mesh`) appended to the compose text before it is saved, and `forge_catalog::stacks::placements` reads a top-level `x-forge.cluster` as the default placement. The saved revision therefore deploys the same way from the console and `forge stacks deploy`. A compose file with its own top-level `x-forge` must agree with the document.
- **Rollout job.** `stack.rollout` deploys the nodes through the ordinary per-node `stack.action` jobs, one at a time and waiting for each (`node-by-node`) or all together (`all-at-once`); a failed node fails the rollout and later nodes are not started. Child jobs carry the idempotency key `stack.rollout:<rollout>:<node>:<op>` and a node already at the revision with reported containers is skipped, so a re-run after a crash starts nothing twice. Nodes the new revision leaves are taken down only after every node is live. A changed `.env` alone also saves a new revision (for every stack update, not only apply), since nodes at the current revision are skipped. The engine re-authorizes `stack.manage` (or `stack.privileged`) before the step and each child job is authorized again. One step loops over the nodes, with the progress in the job's state, rather than one engine step per node, since a job's step list is fixed.
- **Permissions.** `stack.manage` on the environment and `server.manage` on each node, as the stack editor needs; no new Cedar action. Swarm environments are refused (D-171 deploys those as one `docker stack deploy`).
- **Health timeout.** It bounds how long the rollout waits for each node (plus 20 minutes for pulls). The helper's own `docker compose up --wait-timeout` stays at 600 s until the agent takes the value from the command.
- **CLI exit status.** `0` live, `1` document or plan refused, `2` a node failed, `7` a rollout is already running, `8` the wait timed out.

**Why.** Deploying a stack from a repository needs its placement and rollout next to the compose file, without hand-written API calls. Reusing the per-node job keeps one deploy path, one policy check and one audit trail; the rollout only orders them.

**Trade-offs.** The compose text saved by an apply carries the appended `x-forge` block, and a top-level `x-forge` in the compose file cannot be merged with the document's. The health timeout does not yet reach Compose's `--wait-timeout`. Exit `2` for a failed node differs from `forge deploy`'s `10`.

## D-175 Cluster names for stack services and replicas, served by forge-dns on every fabric node

**Decision.** Inside a Default cluster's fabric, every compose stack service with a container port (`ports:` or `expose:`) gets `<svc>.<stack>.<env>.<cluster>.<suffix>` with one A record per running container that is healthy or has no health check, and every running container gets `<svc>-<n>.<stack>.<env>.<cluster>.<suffix>`, `n` being the Compose container number. The addresses are the containers' `mesh_ip`s (D-172), so these rows carry no outside address and reach only nodes of the same fabric. Each Linux fabric node runs forge-dns (D-173) on its mesh address, fed by `/var/lib/forge-dns/names.json` (mode 0644 in a 0755 folder: the agent's and server's own folders are private, and forge-dns runs as its own user, with `ReadOnlyPaths` on the folder so the renamed-in file is seen); the server host runs a second tier when `[dns] listen` is set, writing `[dns] records_file` (the same path by default). The hosts file stays on every node.

- **Coexistence.** A node name still wins over a service name. A replica name equal to a service or stack-service name is dropped and the stack plan says so as an Info finding. Stack and native services with the same name do not collide: the DNS names differ (`api.orders.prod…` against `orders.prod…`), so the existing row-name refusal stays as it is.
- **Sick replicas.** The stack-service name leaves out unhealthy and starting containers; the replica name keeps them for debugging.
- **Freshness.** The agent reads its compose containers through the helper (read-only, unsigned) on every heartbeat and sends `Heartbeat.stack_states` only when they changed, at most every 10 s; a push counts as sent only once its heartbeat succeeds, so a failed heartbeat resends it. The server merges the rows by container name into each stack node's stored state and rebuilds names; ports and image come from the last deploy.
- **Records.** SRV `_<svc>._<proto>.<name>` with the first container port, in forge-dns only. PTR for every A inside the fabric subnet, preferring a replica's or node's name; reverse zones are widened to whole octets.
- **Clients.** Containers on the cluster network get `dns: [<node mesh ip>, <[dns] listen ips>…]` unless they set `dns`, and only when the node's facts say `forge_dns: running`: `dns` replaces Docker's resolver, so without a running forge-dns the containers would resolve nothing. `dns_search` is computed on the node from the zone it receives (`<stack>.<env>.<zone>`, `<env>.<zone>`, `<zone>`), not by the server. The node itself uses forge-dns through interface-scoped `resolvectl` only when systemd-resolved runs; otherwise the hosts file serves it. Windows nodes and nodes outside a fabric keep the hosts file only.
- **Wire.** `HostNames` gains `records_json` (4), `zone` (5) and `upstream_forge_dns` (6); older agents ignore them and keep the hosts file.

**Why.** Container addresses change on every redeploy and scale, and a hosts file cannot carry ports, round robin or reverse names. A resolver on each node keeps lookups local and working when the server is down; the server tier covers a node whose resolver is stopped. Using Compose's container number keeps replica names stable across restarts without a Forge-side slot table.

**Trade-offs.** Native container services still resolve to the node's mesh address. A name follows a change in up to about 10 s (state push) plus 2 s (file poll) plus the 5 s TTL. Agent self-update does not update forge-dns yet; a node gets a newer forge-dns from the package. Nodes without systemd-resolved do not get SRV or PTR answers for their own lookups.

## D-176 Service orchestration in native mode, step 1: a complete, typed container spec

**Decision.** In native mode a Service is the one managed unit (`plans/service-orchestration.md`). Step 1 completes and types its container spec, and adds optimistic locking for edits.

- **Fields.** `forge_catalog::AppSpec` gains:
  - `restart_policy` (`no`, `always`, `unless-stopped` or `on-failure`) and `restart_max_attempts`;
  - `cap_add` and `cap_drop`;
  - `ulimits`, `tmpfs`, `dns`, `sysctls` and `devices`;
  - `init` and `networks`.

  Their types and checks live in `forge_catalog::container_opts`. The old boolean `restart` is still read and decides when `restart_policy` is absent. A native Service maps the policy onto systemd `Restart=`. With `on-failure`, it also gets `StartLimitBurst=n+1` in a one-day window, because systemd counts the first start too. The agent runs `systemctl reset-failed` before every deploy and operator start, so only automatic restarts count. Where there is no systemd, Forge's supervisor applies the same rule: it restarts only after a non-zero exit, and at most `n` times.
- **Wire compatibility.** Agents refuse unknown spec fields, so every new field is left out when empty. A spec that uses one is sent only to an agent that reports the `container_options` fact; otherwise the deploy step fails and names the cause. A failed facts read, or a node that has not reported facts yet, is retried rather than treated as an older agent.
- **Policy.** Added capabilities, devices and sysctls need `stack.privileged`, as on the stack path. They join host-path volumes in the job's `privileged_mounts` list (`cap_add:X`, `device:/dev/x`, `sysctl:k`), so one approval covers exactly what was shown, and the job re-checks it at dispatch.
- **Storage.** The API writes `services.spec` through one typed struct (`StoredSpec`), not an ad hoc `json!`.
- **Locking.** Migration 0005 adds `services.version`, and every save bumps it. `PUT /services/{id}` with `version` saves only when the stored version still matches; otherwise it answers `409` "this service changed since you opened it". A PUT without `version` (older clients) still saves. The console and `forge services update` send the version they read.

**Why.** Swarm parity in native mode (later steps: reconciler, Compose-format apply) needs every common container setting as a typed value that the console, the CLI and a file can all describe. Typed values let the policy check each setting, and the version stops two editors from silently overwriting each other.

**Trade-offs.** Two fields describe restarts during the transition (`restart` and `restart_policy`). The domain `Service.spec` is still a `Value`; only the API's write path is typed. Extra networks must already exist on the node, because Forge does not create them.
