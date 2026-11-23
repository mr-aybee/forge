---
title: Architecture
description: Components, trust boundaries, crate layout and dependency rules.
sidebar:
  order: 1
---

Forge is one control-plane binary, a plugin runner spawned on demand, and one
small agent per managed server. Agents always dial out, so managed servers
need no inbound ports.

## Components

| Process | Responsibility | Trust |
| --- | --- | --- |
| `forge-server` | REST API, WebSocket events, identity, authorization, registry, jobs, audit, agent hub, plugin host | Highest: holds fleet state |
| `forge-analytics-worker` | All analytics: spool uploads from agents, log and telemetry ingest, node and container history, log searches, retention, and the embedded engine. A child of `forge-server`, which restarts it when it exits, hangs or passes its memory limit. See [Analytics](/docs/concepts/analytics/#the-analytics-worker-process) | Trusted; reached by `forge-server` over a private socket with a per-start secret, and by agents over mTLS on port `55132` |
| `forge-plugin-runner` | Executes one WebAssembly component under budgets; exists only while a plugin runs | Untrusted code |
| `forge-agent` | Dials the control plane, verifies signed commands, reports facts. Keeps the node's analytics spool, runs the local log listener on port `5341` and uploads the spool to the analytics worker | Trusted for its own server only |
| `forge-agent-helper` | Performs the few privileged operations from a fixed allowlist | Root/SYSTEM, minimal surface |

## Trust boundaries

```diagram
alt: Forge components and connections
flow:
  rows:
    - title: Operators
      steps:
        - { label: Browser / Tauri app, edge: "HTTPS: cookie+CSRF or bearer (PKCE)" }
        - { label: control plane, tone: accent }
    - title: Nodes
      steps:
        - { label: control plane, tone: accent, edge: "outbound-only HTTP/2 + protobuf, mTLS, signed cmds" }
        - { label: agent, edge: "" }
        - { label: helper, note: allowlist }
    - title: Analytics
      steps:
        - { label: apps on the node, edge: "X-Seq-ApiKey, :5341 (loopback, Docker bridge)" }
        - { label: agent spool, edge: "outbound-only HTTP/2, mTLS :55132" }
        - { label: analytics worker, tone: accent }
    - title: Plugins
      steps:
        - { label: control plane, tone: accent, edge: stdio JSON lines }
        - { label: plugin runner, note: "no WASI, budgets" }
```

1. **Browser ↔ control plane** — only an HttpOnly session cookie reaches the
   browser; mutations need a CSRF token. Apps use OAuth + PKCE and bearer
   tokens. No secrets in URLs or logs (request logs record paths only).
2. **Control plane ↔ agent** — mutual TLS with per-agent certificates from
   the internal CA. Every command is Ed25519-signed, bound to one agent,
   time-limited and replay-checked.
3. **Control plane ↔ plugin runner** — separate process, no filesystem or
   network imports. Host calls go back to the control plane for checking.
4. **Agent ↔ helper** — a closed set of typed requests with validated arguments.
5. **Apps ↔ agent** — the local log listener binds only loopback and the
   Docker bridge gateway, and accepts only ingest keys of services placed
   on that node.
6. **Agent ↔ analytics worker** — mutual TLS with the agent's certificate;
   the worker takes the node from the certificate and trusts nothing in a
   spool file that names a node or a service. See
   [Node spools](/docs/concepts/analytics/#node-spools).

## Source layout

All code lives under `src/`, grouped by architectural role:

| Directory | Crates | Role |
| --- | --- | --- |
| `src/domain` | `forge-core` | Types, typed ids, access vocabulary, **ports** (traits). No I/O. |
| `src/services` | `forge-authz`, `forge-audit`, `forge-identity`, `forge-jobs`, `forge-pki`, `forge-secrets`, `forge-plugin-host` | Business rules. Depend only on ports. |
| `src/adapters` | `forge-store` | Implementations of ports (SQL). |
| `src/protocol` | `forge-proto`, `forge-sdk` | Wire contracts shared with agents and plugin authors. |
| `src/apps` | `forge-server`, `forge-analytics-worker`, `forge-agent`, `forge-plugin-runner` | Binaries and composition roots. |
| `src/tools` | `fleet-sim` | Benchmarks. |
| `src/plugins` | `uptime-probe` | Sample plugin. |
| `src/ui` | React app | Web UI, later bundled into Tauri apps. |

### Dependency rule

Dependencies point inward: apps → services → domain, and adapters → domain.
Services never name a concrete adapter; `forge-server/src/state.rs` is the
only place concrete types are chosen and injected. This keeps services
testable with fakes and lets later phases add adapters (PostgreSQL, KMS,
OpenBao) without touching business logic.

## Lightweight by design

- One process and SQLite in the small profile; no Redis, broker or microservices.
- The plugin runner costs nothing when no plugin is running.
- The agent uses a single-threaded runtime and one HTTP/2 connection.
- Agents long-poll, so an idle fleet costs one parked request per agent.

Measured numbers are in [Benchmarks](/docs/development/benchmarks/).
