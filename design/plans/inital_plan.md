
# Orbyn Forge — Platform Plan

## 1. Product definition

Orbyn Forge is a Rust orchestration layer that gives one secure workspace to access, deploy and operate applications across Windows and Linux. It manages proven engines (Docker, Swarm, K3s/Kubernetes, Caddy, PowerDNS, guacd, database servers) instead of rewriting them.

**Core promise:** one login, one permission model, one audit trail, one application view, across every server, runtime and environment.

**What it is not:** a container runtime, a Kubernetes distribution, a DNS server, an RDP implementation or a database engine. It adopts and drives those.

### Target customers

| Segment                                         | Pain today                                      | Why Forge                                                    |
| ----------------------------------------------- | ----------------------------------------------- | ------------------------------------------------------------ |
| Small hosting providers and MSPs                | aaPanel/cPanel per server, no multi-tenant view | Multi-server, customer tenants, scoped access                |
| SMB IT teams with mixed Windows + Linux         | IIS, RDP, SSH and Linux panels all separate     | One Windows/Linux workspace                                  |
| Dev teams outgrowing Coolify/Heroku-style tools | Need native services, Windows, staged releases  | Immutable releases, promotion, blue-green                    |
| Agencies hosting many client apps               | Shared credentials, no audit                    | Per-client tenants, browser access without sharing passwords |
| Regulated or offline sites                      | SaaS tools not allowed                          | Self-hosted, offline installs, signed updates                |

### Smallest coherent version (MVP)

Enroll Windows and Linux servers, open browser SSH/RDP without sharing passwords, deploy a .NET or Node app as a systemd service or Windows Service with blue-green through Caddy, with SSO, roles and audit. That alone beats every single panel listed in the brief.

## 2. Reuse strategy: orchestrate, don't fork

Forge reuses open-source tools in three ways, chosen per tool by license and language. Copying code into the Rust core is the rarest case, because most panels are PHP, Python, Go or Java and many are GPL/AGPL.

1. **Run as a managed engine** (default). Forge installs, configures and drives the tool through its API or CLI as a separate process: Caddy, PowerDNS, guacd, K3s, Docker, Adminer. License obligations stay with that tool.
2. **Learn from, then reimplement in Rust.** Read how Coolify, CyberPanel or aaPanel handle a problem (Nginx vhost templates, SSL flows, PHP-FPM pools), port the behaviour and their edge cases into our own tests. Ideas and behaviour are not copyrighted; copied code is.
3. **Vendor code directly** only from permissive licenses (MIT, Apache-2.0, BSD), with attribution and an SBOM entry.

**Rule:** no GPL/AGPL code is copied into the core, agent or SDK. Doing so would put the whole platform under that license. Running GPL tools as separate processes is fine.

### License snapshot (approximate, verify each before use)

| Tool                     | License (from memory)                  | How Forge uses it                               |
| ------------------------ | -------------------------------------- | ----------------------------------------------- |
| Coolify                  | Apache-2.0                             | Study deployment flows; reuse ideas, maybe code |
| Rancher, K3s, RKE2       | Apache-2.0                             | Managed engine; study cluster import            |
| Portainer CE             | zlib                                   | Study Docker/Swarm UX                           |
| Apache Guacamole (guacd) | Apache-2.0                             | Managed engine                                  |
| IronRDP                  | MIT / Apache-2.0                       | Rust library, can embed                         |
| Caddy                    | Apache-2.0                             | Managed engine                                  |
| Traefik                  | MIT                                    | Managed engine (plugin)                         |
| HAProxy                  | GPLv2                                  | Managed engine only                             |
| PowerDNS                 | GPLv2                                  | Managed engine only                             |
| CyberPanel               | GPLv3                                  | Study only                                      |
| aaPanel                  | Unclear, check terms                   | Study only                                      |
| phpMyAdmin               | GPLv2                                  | Optional plugin, separate process               |
| pgAdmin                  | PostgreSQL License                     | Optional plugin                                 |
| CloudBeaver              | Apache-2.0                             | Optional plugin (heavy, Java)                   |
| Redis 8                  | AGPL / RSAL / SSPL choice              | Managed engine; offer Valkey (BSD) too          |
| Teleport                 | Changed licensing; check current terms | Optional integration only                       |

### Testing reused behaviour

- Build a **conformance test suite per engine**: given a Forge config, the engine's real config and live behaviour must match.
- Port interesting edge cases from other panels' issue trackers into those tests.
- Run against real engines in containers or VMs in CI, per supported OS and version.

## 3. Tech stack

Everything Forge itself runs is Rust; the UI is React, shipped in the browser and inside Tauri apps for desktop and mobile.

| Layer           | Choice                                                        | Why                                                   |
| --------------- | ------------------------------------------------------------- | ----------------------------------------------------- |
| Language        | Rust (stable), Tokio                                          | Speed, memory safety, small static binaries           |
| HTTP API        | Axum + Tower + Hyper                                          | Thin, fast, middleware for auth, limits, tracing      |
| Agent link      | HTTP/2 + protobuf (tonic, or prost + h2) over mTLS            | Outbound-only, streaming, typed                       |
| TLS / PKI       | rustls, rcgen                                                 | Internal CA, per-agent certs, no OpenSSL              |
| Database        | SQLite (WAL) small; PostgreSQL large; via sqlx                | No extra server in the small profile                  |
| Jobs            | Durable job table with leases + idempotency keys              | No Redis or broker                                    |
| Authorization   | Cedar (embedded policy engine)                                | Resource-scoped, auditable, testable policies         |
| Identity        | openidconnect, webauthn-rs, argon2                            | SSO, passkeys/MFA, local accounts                     |
| Secrets         | Envelope encryption (age / ring); OpenBao plugin              | Encrypted at rest, released per job                   |
| Linux control   | zbus (systemd D-Bus), nix                                     | No shelling out for services                          |
| Windows control | windows-rs (SCM, Event Log, ETW, perf counters)               | Native Windows management                             |
| Containers      | bollard (Docker/Swarm), kube-rs (Kubernetes)                  | Mature Rust clients                                   |
| Plugins         | Wasmtime + Component Model (WIT) in a separate runner process | Typed, sandboxed, fuel and deadlines                  |
| SSH gateway     | russh                                                         | Pure Rust, light                                      |
| RDP             | IronRDP first; guacd fallback; guacd for VNC                  | Rust where possible                                   |
| Certificates    | instant-acme                                                  | ACME for engines without built-in ACME                |
| Observability   | tracing, built-in small metrics store; VictoriaMetrics plugin | Low memory by default                                 |
| Web UI          | React + TypeScript + Vite, TanStack Query/Router              | Huge ecosystem, xterm.js and RDP canvas are JS anyway |
| UI kit          | Radix UI primitives + Tailwind, own design system             | Accessible, light, themeable                          |
| Desktop/mobile  | Tauri v2 (Windows, macOS, Linux, iOS, Android)                | Reuses the React UI, small apps                       |
| CLI             | clap, same API client crate as the UI backend                 | One API surface                                       |
| Build           | cargo, cross, musl static, LTO, panic=abort                   | Small binaries, simple installs                       |

### Multi-panel apps (Tauri)

- The UI is **bundled in the app**; it never loads a panel's remote HTML, so remote pages can't reach native app functions.
- Each **profile** = name, URL, pinned TLS fingerprint, auth tokens in the OS keychain.
- Login in apps uses **OAuth + PKCE through the system browser**; the web panel uses HttpOnly cookies + CSRF. The backend supports both.
- Every panel exposes `/api/version` and a capabilities list so one app works with panels of different versions.
- Push alerts go through an optional relay service, since self-hosted panels can't reach Apple/Google push directly.

## 4. Architecture and trust boundaries

Forge is one control-plane binary plus two separately isolated processes (access gateway, plugin runner) and one small agent per server; agents always dial out, so managed servers need no inbound ports.

&#91;embedded content: architecture · 3 trust zones\]

Dashed zones are trust boundaries; engine admin APIs (Docker socket, Caddy, PowerDNS, guacd) are reached only by the local agent.

| Component                | Responsibility                                               | Trust level                                     |
| ------------------------ | ------------------------------------------------------------ | ----------------------------------------------- |
| Control plane            | API, auth, policy, registry, jobs, audit, secrets, agent hub | Highest; holds fleet state                      |
| Access gateway           | Browser SSH/RDP/VNC sessions, recording                      | Separate process or host; no stored credentials |
| Plugin runner            | Wasm plugins under budgets                                   | Untrusted code; least privilege                 |
| Build workers (optional) | Image and artifact builds                                    | Untrusted input; isolated hosts                 |
| Agent                    | Executes signed, scoped commands on one server               | Trusted only for its own server                 |
| Engines                  | Proxies, DNS, containers, DBs                                | Managed; private endpoints only                 |

Larger installs split the control plane into API and worker processes on PostgreSQL and run several gateways; the code stays one codebase.

## 5. Module and capability map

Six modules form the mandatory core; everything else is a plugin that is installed, enabled and versioned separately.

### Core (always present)

| Module            | Capabilities                                                                                    |
| ----------------- | ----------------------------------------------------------------------------------------------- |
| Identity          | Local users, OIDC SSO, passkeys/TOTP MFA, service identities, API tokens, sessions              |
| Authorization     | Cedar policies, roles, resource scopes, temporary access, approvals                             |
| Resource registry | Orgs, tenants, projects, environments, servers, apps, clusters, databases, typed and extensible |
| Jobs and audit    | Durable jobs, steps, retries, idempotency, append-only signed audit log                         |
| Plugin host       | Install, verify, enable, upgrade, pin, quarantine, budgets                                      |
| Secure comms      | Internal CA, agent enrollment, mTLS, cert rotation and revocation                               |

### Plugin domains

| Domain          | First engine                         | Later engines                                                       |
| --------------- | ------------------------------------ | ------------------------------------------------------------------- |
| Server adapters | Ubuntu LTS, Windows Server 2022/2025 | Debian, RHEL family, Alpine                                         |
| Native services | systemd, Windows SCM                 | Supervisor-style process mode                                       |
| Remote access   | SSH (russh), RDP (IronRDP/guacd)     | VNC, Teleport, hypervisor consoles (Proxmox, vSphere, IPMI/Redfish) |
| Reverse proxy   | Caddy                                | Traefik, Nginx, HAProxy, IIS ARR coexistence                        |
| DNS             | PowerDNS Authoritative               | Cloudflare, Route 53, others via provider API                       |
| Certificates    | Caddy ACME / instant-acme            | Internal CA certs, imported certs                                   |
| Containers      | Docker Engine                        | Podman                                                              |
| Clusters        | Docker Swarm, K3s                    | Full Kubernetes (RKE2/kubeadm), managed EKS/AKS/GKE import          |
| Databases       | PostgreSQL, MySQL/MariaDB            | SQL Server, Redis/Valkey, MongoDB                                   |
| DB admin GUIs   | Built-in query console, Adminer      | pgAdmin, phpMyAdmin, CloudBeaver                                    |
| Backups         | Restic to S3/local                   | Engine-native backups, Windows VSS                                  |
| Monitoring      | Built-in metrics + logs              | VictoriaMetrics, Prometheus, Loki, OpenTelemetry                    |
| Deployments     | Artifact + image deploys from CI     | Hosted builds (Nixpacks/Buildpacks-style)                           |
| Notifications   | Email, webhook                       | Slack, Teams, Telegram, push relay                                  |
| Hosting extras  | PHP-FPM sites, static sites          | Mail hosting, FTP/SFTP accounts, WordPress tooling                  |
| Secrets         | Built-in vault                       | OpenBao/Vault, cloud KMS                                            |
| Custom          | Declarative modules                  | Customer Wasm plugins, external services                            |

### Cross-cutting features

- **Adopt existing infrastructure:** discover running services, sites, containers and databases; register them without migrating.
- **Preview before apply:** every change shows a diff of the config it will write.
- **Drift detection:** compare desired vs actual; adopt or revert.
- **Unified application view** across systemd, Windows Services, containers and clusters.
- **Offline installs:** signed bundles of the platform, agents and plugins.

## 6. Container orchestration

Users pick one of four container tiers per environment, from a single Docker host up to full Kubernetes. Your two Kubernetes "flavours" are right: **K3s** is the lightweight one, and **RKE2 or kubeadm upstream Kubernetes** is the heavier, fuller one.

| Tier                | Engine                                            | Rough control-plane RAM (approx.) | Best for                                                   | Forge's role                                |
| ------------------- | ------------------------------------------------- | --------------------------------- | ---------------------------------------------------------- | ------------------------------------------- |
| 1. Single host      | Docker Engine                                     | \~100 MB daemon                   | One server, simple apps                                    | Deploy, logs, volumes, networks             |
| 2. Simple cluster   | Docker Swarm                                      | Little on top of Docker           | 2–20 nodes, small teams                                   | Services, stacks, secrets, rolling updates  |
| 3. Light Kubernetes | K3s (single binary, SQLite or embedded etcd)      | \~500 MB–1 GB server node        | Edge, small clusters, Kubernetes skills without the weight | Install, upgrade, deploy, observe           |
| 4. Full Kubernetes  | RKE2 (hardened) or kubeadm; or import EKS/AKS/GKE | 2 GB+ per control-plane node      | Large or regulated workloads                               | Import, deploy, observe; provisioning later |

The RAM figures are rough expectations to measure, not benchmarks. Also consider **k0s** as a K3s alternative and **Talos Linux** as an immutable OS for full clusters.

### Design rules

- **The orchestrator owns scheduling.** Forge submits desired state (Swarm stacks, Kubernetes manifests or Helm releases) and watches status. It never places containers itself on Swarm or Kubernetes.
- **One application model, many targets.** An app component declares image, ports, env, volumes, health checks and replicas; a per-tier translator emits Docker run specs, Swarm stacks or Kubernetes Deployments/Services/Ingress.
- **Show tier differences.** Canary by weight needs an ingress that supports it; Swarm lacks native canary, so Forge does it with two services behind the proxy.
- **Import first.** Existing Swarm and Kubernetes clusters are imported read-only, then promoted to managed.
- **Kubernetes access** uses a kubeconfig held server-side, scoped service accounts per tenant, and namespace-per-environment by default.
- **Upgrade paths:** Docker → Swarm needs no app changes; Swarm → K3s goes through the translator; K3s → RKE2 keeps the same manifests.
- **Windows containers** are supported on Windows hosts and in Kubernetes Windows node pools only; Swarm support for them is limited.

### Phasing

Docker first, then Swarm, then K3s install and deploy, then full-cluster import. Provisioning full production Kubernetes is its own workstream, not a dashboard feature.

## 7. Deployment, rollback and availability

A deployment is a durable, resumable job that moves one immutable release to one environment; deployment strategy and availability strategy are separate settings.

### Application model

Application → environments (dev, UAT, staging, prod) → components (frontend, API, worker, cron) → instances on targets (systemd, Windows Service, Docker, Swarm, Kubernetes). A **release** is an immutable set of artifacts identified by digest; promotion moves the same digest between environments.

### Deployment job steps

1. Validate permissions, compatibility, dependencies and capacity.
2. Fetch artifact and verify its digest and signature.
3. Render config and resolve secrets on the target only.
4. Start candidate instances (new release dir, new port or new service name).
5. Readiness and smoke checks.
6. Shift traffic at the proxy (all at once, rolling or weighted).
7. Observe errors, latency and health for a set window.
8. Finalize, or restore the previous route and release.

Each step writes its state to the job table, so a crashed control plane resumes or rolls back from the last recorded step.

### Strategies by target

| Strategy   | systemd / Windows Service          | Docker                   | Swarm                  | Kubernetes                      |
| ---------- | ---------------------------------- | ------------------------ | ---------------------- | ------------------------------- |
| Recreate   | Yes                                | Yes                      | Yes                    | Yes                             |
| Rolling    | Across servers                     | Across hosts             | Native                 | Native                          |
| Blue-green | Two release dirs + ports via proxy | Two containers via proxy | Two services via proxy | Two Deployments, Service switch |
| Canary     | Proxy weights                      | Proxy weights            | Proxy weights          | Ingress/Gateway weights         |

### Availability

- **Active-active:** replicas behind a proxy pool.
- **Active-standby:** standby workers take a **lease** (stored in the control plane or the app's DB) before running; fencing tokens stop duplicate jobs.
- **Gateway redundancy:** two proxies with a floating IP (keepalived on Linux) or an external load balancer; otherwise the proxy is a single point of failure.

### Hard limits Forge must state in the UI

- Rollback does not undo database migrations; Forge supports expand/contract migrations and a separate migration step with its own approval.
- Stateful services (databases, queues) use engine-specific recovery, not generic rollback.
- Zero downtime needs connection draining, spare capacity and backward-compatible data changes.
- IIS: ASP.NET Core apps can move to Kestrel behind Caddy; legacy System.Web apps stay on IIS, and Forge manages IIS sites and app pools through a Windows adapter.

## 8. Browser access, proxy, DNS and databases

All four follow one pattern: Forge holds credentials and config server-side, checks permission per action, and talks to private engine endpoints that are never public.

### Browser access (SSH, RDP, VNC)

1. User authenticates (SSO + MFA).
2. Cedar authorizes the target server **and** the OS account.
3. Step-up MFA or approval if policy requires it.
4. Forge issues a single-use session ticket valid for \~60 seconds.
5. Browser opens a WebSocket to the **access gateway**, which redeems the ticket.
6. Gateway reaches the server through the agent's outbound tunnel (no inbound SSH/RDP ports needed) and injects the credential from the vault.
7. Audit records start, end, user, target; optional recording (SSH as asciicast, RDP via guacd recording).
8. Expiry, idle timeout and admin revoke close the session immediately.

Clipboard and file transfer are per-policy. OS sessions are not recovery consoles; boot-level access needs hypervisor or IPMI/Redfish plugins.

### Reverse proxy

- Proxy **pools** (public prod, private, UAT, per-customer, per-site), each with one or more instances.
- Config is generated, versioned and applied as: Validate → Stage → Apply → Verify → Revert on failure.
- Each instance reports its **applied config version**; the UI shows drift and lagging instances.
- Caddy first (JSON admin API on localhost only, reached by the agent). Traefik/HAProxy/Nginx later.
- Proxies keep serving with their last config if the control plane is down.

### DNS

- PowerDNS Authoritative via its HTTP API (private), plus cloud DNS provider plugins.
- Zone-scoped permissions, record templates, deploy-driven record changes, full audit.
- DNSSEC through PowerDNS's own key handling, enabled per zone.
- DNS is not failover: TTL caching delays changes. Use a floating IP or external load balancer for fast failover.

### Databases

Two separate capabilities with separate permissions:

- **Service management:** discover, provision, start/stop, users and grants, config, backups and restore tests, versions, replication status.
- **Data access:** a built-in Rust query console (sqlx, tiberius for SQL Server, fred for Redis) that opens short-lived, least-privilege DB users per session; GUIs (Adminer, pgAdmin, phpMyAdmin, CloudBeaver) as optional plugins behind the access gateway.

Rules: DB endpoints stay private; credentials never reach the browser, URLs or local storage; SQL access is flagged as sensitive because it can bypass app rules; an embedded GUI never gets a broader DB user than the Forge user is allowed.

## 9. Plugin system

Every integration is a signed, versioned plugin with a manifest; plugins get typed host functions, never raw shell or the main UI's credentials.

### Three extension models

| Model                | Runs as                                         | Use for                                                       |
| -------------------- | ----------------------------------------------- | ------------------------------------------------------------- |
| Declarative module   | TOML/JSON, no code                              | Forms, dashboards, templates, recipes, health checks, actions |
| Wasm plugin          | Wasmtime component in the plugin-runner process | Connectors, validators, policies, custom logic                |
| External integration | Separate service via versioned API + events     | Long-running or heavy tools (DB GUIs, build systems)          |

### Manifest (`forge-plugin.toml`)

ID and publisher · semver version · supported Forge API range · dependencies with ranges · OS/arch/runtime needs · config schema · UI contributions · actions and events · requested permissions and resource scopes · execution location (central, worker, node) · CPU/memory/time/concurrency budgets · migration hooks · signature and checksums.

### Lifecycle

Discover/upload → inspect (publisher, permissions, deps) → install → configure → enable → upgrade (staged per worker/node) → pin → roll back (if migrations allow) → disable → uninstall.

- Track **installed, desired and active** version per node.
- Sources: public catalog, private repos, org catalogs, offline bundles.
- Disabling stops new runs; running jobs finish or cancel per policy; managed resources are never deleted.
- Uninstall shows dependents and separates "remove integration" from "delete infrastructure".
- Revocation list can quarantine a plugin version everywhere.

### Security

- Effective permission = user's permission ∩ plugin's approved permission ∩ resource policy.
- Host functions are typed ("restart service X", "write proxy route"), each checked by Cedar.
- Limits: network allowlist, explicit file paths, scoped secret references, Wasmtime fuel + epoch deadlines, memory caps, output caps, cgroup/Job Object limits on the runner process.
- Failure throttling and auto-quarantine after repeated faults.
- Inactive plugins have zero running instances.
- A signature proves who published it, not that it is safe; admins approve permissions explicitly.

### UI contributions

Default: schema-driven (forms, tables, charts) rendered by Forge's React UI. Custom UIs run in a sandboxed iframe on a separate origin with a postMessage API and no access to user tokens.

## 10. Security model

Every request is authorized at the place it takes effect (API, job runner, agent, gateway, plugin host), so a hidden button is never the only protection.

### Identity and access

- SSO (OIDC), passkeys and TOTP MFA; step-up MFA for terminals, secrets and production changes.
- Roles: platform admin, org admin, infra operator, developer, DBA, auditor, customer.
- Separate permissions: view, deploy, restart, edit config, read secrets, open terminal, administer DB, manage infra.
- Just-in-time access with expiry and optional approval.
- Root/Administrator sessions are labelled privileged: they can bypass every app-level rule, so they need stronger policy and recording.

### Agents

- Enrollment by one-time token + server-generated key; CA signs a unique client cert.
- Outbound-only mTLS connection; short-lived certs with automatic rotation; revocation list.
- Every command carries an operation ID, signature and expiry; the agent rejects replays and re-checks scope.
- Agent runs privileged steps in a separate helper with a fixed command allowlist; ordinary work runs unprivileged.
- Updates are signed; the agent verifies before replacing itself and keeps the previous binary for rollback.

### Secrets

- Envelope encryption; master key from a file, TPM, cloud KMS or OpenBao.
- Secrets are released only to the job or agent that needs them, never to the browser.

### Containment

| Compromised part | What it can reach           | What stops it spreading                                             |
| ---------------- | --------------------------- | ------------------------------------------------------------------- |
| User account     | That user's scopes          | MFA, step-up, JIT expiry, audit alerts                              |
| Plugin           | Its approved host functions | Permission intersection, sandbox, quarantine                        |
| Agent / server   | That server only            | Per-agent cert, signed commands, no fleet credentials on nodes      |
| Access gateway   | Active sessions through it  | Single-use tickets, no stored credentials, separate host option     |
| Control plane    | Everything                  | Admin MFA, separate gateway, signed agent commands, offline backups |

### Platform hygiene

- Docker socket, PowerDNS API, proxy admin APIs and guacd bound to localhost or private networks.
- Append-only, hash-chained audit log, exportable to external storage.
- Encrypted backups of platform state with restore tests.
- SBOM, `cargo audit`/`cargo deny`, reproducible signed releases.
- Rust prevents memory bugs, not authorization or design mistakes; threat modelling and pen tests stay required.

## 11. User interface and feel

The UI should feel like a calm, fast operations console (closer to Linear or Vercel than to cPanel): application-first, keyboard-driven, and honest about what each runtime can do.

### Principles

- **Applications first, servers second.** Home shows apps and their health; servers, clusters and proxies are one click away.
- **One scope bar** at the top: profile (panel) → organization → project → environment. Everything below filters to it.
- **Command palette (Ctrl/Cmd+K)** for every action: "open SSH to web-01", "promote api 1.4.2 to prod".
- **Preview, then apply.** Every change shows a diff and the exact targets before running.
- **Jobs are visible.** A jobs drawer shows live steps, logs and a resume/rollback button.
- **Runtime honesty.** Unsupported actions show why ("Canary needs a weighted proxy") instead of disappearing.
- **Fast.** Under 200 KB of initial JavaScript (gzip) as a target, code-split per module, optimistic updates, live state over one WebSocket.
- Dark and light themes, dense and comfortable layouts, full keyboard and screen-reader support.

### Main screens

| Screen      | Contents                                                                                       |
| ----------- | ---------------------------------------------------------------------------------------------- |
| Overview    | App health, failing deploys, expiring certs, offline agents, pending approvals                 |
| Application | Environments as columns, releases, components and their targets, promote button, domains, logs |
| Servers     | Fleet table, OS, agent version, load, services; open terminal/desktop                          |
| Clusters    | Swarm and Kubernetes: nodes, workloads, events                                                 |
| Networking  | Proxy pools, routes, certificates, DNS zones                                                   |
| Databases   | Instances, databases, users, backups, open console                                             |
| Access      | Live sessions, recordings, JIT requests                                                        |
| Plugins     | Catalog, installed, versions per node, resource cost, health                                   |
| Settings    | Identity, roles, policies, audit log                                                           |

### Sessions inside the UI

Terminals and remote desktops open as tabs in a workspace pane (like an IDE), so users can keep several servers open and still navigate the app.

### Desktop and mobile apps

- Profile switcher lists all panels with status dots; one tap switches.
- Mobile focuses on overview, alerts, approvals, restarts, logs and SSH; RDP is available but secondary.
- Biometric unlock for stored profiles.

## 12. Lightweight profile and benchmarks

The small profile runs the whole core on 2 vCPU / 2 GB RAM with room to spare; these are acceptance targets to prove by benchmark, not measured results.

| Component                          | Idle RAM target     | Idle CPU target        |
| ---------------------------------- | ------------------- | ---------------------- |
| Node agent                         | ≤ 25 MB RSS        | < 1% of one core (avg) |
| Control plane + SQLite, 50 servers | ≤ 150 MB           | < 3% of one core       |
| Access gateway, idle               | ≤ 30 MB            | \~0                    |
| Plugin runner, no active plugins   | 0 (not running)     | 0                      |
| Per SSH session                    | ≤ 5 MB             | —                     |
| Per RDP session (IronRDP or guacd) | tens of MB, measure | —                     |

Third-party engines (K3s, databases, Caddy, DB GUIs, metrics stores) are reported separately and never counted as "Forge".

### Rules that keep it light

- Installed ≠ running; plugin instances start on demand and stop when idle.
- Agents push changes (inotify, D-Bus signals, ETW) instead of polling; metrics every 30–60 s by default, faster only when a user is watching.
- Jitter + exponential backoff on reconnects to survive reconnect storms.
- Bounded queues, global and per-plugin concurrency limits, backpressure.
- Logs streamed on demand; local ring buffers with fixed size.
- Agents keep running workloads untouched if the control plane is down.

### Benchmark plan

1. **Fleet simulator:** a Rust tool that fakes 10, 100, 1,000 and 5,000 agents.
2. Measure control-plane RSS, CPU, p99 API latency and DB size at each size.
3. Reconnect storm: all agents drop and return in 10 s; record recovery time.
4. Real agent on Ubuntu and Windows Server VMs: idle RSS and CPU over 24 hours.
5. Deploy throughput: 100 parallel blue-green deploys on the small profile.
6. Run in CI on every release; fail the build if a target regresses by more than 10%.

## 13. Phased roadmap

Nine phases ship in three releases; the MVP (phases 1–3) already replaces separate SSH/RDP tools and simple panels for mixed Windows and Linux fleets.

&#91;embedded content: roadmap · 9 phases in 3 releases\]

Each phase depends on the one above it; extension contracts for later domains are designed in phase 1 but implemented only when their phase arrives.

| Phase                  | Acceptance criteria (gate)                                                                                                                                       |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1. Core foundation     | Cedar denies an unauthorized action at API, job and agent level in tests; audit log is hash-chained; a signed Wasm plugin installs, runs and is quarantined      |
| 2. Enrollment + access | Ubuntu and Windows Server agents enroll in under 2 minutes; browser SSH and RDP work without credentials reaching the browser; sessions revoke within 5 s        |
| 3. Native deployments  | A .NET API deploys to systemd and a worker to Windows Service; same artifact digest promoted dev → prod; interrupted deploy resumes after control-plane restart |
| 4. Proxy + blue-green  | Blue-green switch with zero failed requests in a load test; bad config auto-reverts; every proxy instance reports its applied version                            |
| 5. DNS + databases     | Deploy updates DNS records with audit; Postgres/MySQL users and backups managed; query console uses short-lived DB users                                         |
| 6. Reliability         | Secrets rotation, scheduled backups with automatic restore tests, control-plane restore from backup in under 30 min                                              |
| 7. Docker + Swarm      | One application spans systemd, Windows Service and Swarm; Swarm stack rolling update and rollback                                                                |
| 8. Kubernetes          | K3s installed by Forge; existing cluster imported read-only then managed; canary via ingress weights                                                             |
| 9. Ecosystem           | Private plugin catalog, offline bundles, staged plugin rollouts per node, SAML/SCIM for enterprise                                                               |

The agent and control-plane resource targets from section 12 are checked at every gate.

## 14. Risks, open decisions and ideas

The biggest risk is scope: each plugin domain is a product on its own, so the roadmap ships one engine per category and proves it before adding more.

### Major risks

| Risk                                  | Impact                  | Mitigation                                                   |
| ------------------------------------- | ----------------------- | ------------------------------------------------------------ |
| Scope creep across 15+ domains        | Nothing ships well      | Strict phase gates; one engine per category                  |
| Windows parity is harder than Linux   | Weak Windows story      | Windows adapter in Phase 2, dedicated Windows test lab       |
| IronRDP maturity for all RDP features | Broken desktop sessions | guacd fallback behind the same gateway API                   |
| Tauri mobile plugin gaps              | Weaker mobile app       | Early proof of concept; web app works on mobile meanwhile    |
| GPL/AGPL contamination                | Forced relicensing      | License checks in CI (`cargo deny`), process-boundary rule |
| Kubernetes depth                      | Endless edge cases      | Import and deploy first; provisioning later                  |
| Control plane is a high-value target  | Fleet compromise        | Section 10 controls, external pen tests                      |

### Open decisions

- [ ] Product license: open-core (Apache-2.0 core + commercial enterprise features) vs AGPL vs source-available.
- [ ] tonic vs custom HTTP/2 framing for the agent (measure memory first).
- [ ] IronRDP vs guacd as the default RDP path.
- [ ] Built-in metrics store vs requiring VictoriaMetrics for history beyond 7 days.
- [ ] Hosted builds: Nixpacks/Buildpacks integration or CI-only.
- [ ] Name check for "Orbyn Forge" (trademark, domains, package names).
- [ ] Which Linux distributions are supported at GA.

### Ideas backlog

- Terraform/OpenTofu provider and GitOps mode (desired state from a Git repo).
- Cost view per application across clouds.
- AI assistant that explains failed deploys from logs and job history.
- Status page generated from health checks.
- Marketplace for community plugins with security ratings.
- WordPress/PHP hosting pack for hosting providers.
- Mail hosting plugin (Stalwart, Rust-based) as a separate workstream.
