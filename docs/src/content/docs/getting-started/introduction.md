---
title: Introduction
description: What Orbyn Forge is, what it is not, and what it does today.
sidebar:
  order: 1
---

Orbyn Forge is a Rust orchestration layer that gives one secure workspace to
access, deploy and operate services across Windows and Linux. It drives
proven engines, such as Docker, Nginx, PowerDNS and database
servers, instead of reimplementing them.

**It is not** a container runtime, a Kubernetes distribution, a DNS server, an
RDP implementation or a database engine.

## Who it is for

| Segment | Why Forge |
| --- | --- |
| Hosting providers and MSPs | Multi-server, customer tenants, scoped access |
| SMB IT with mixed Windows + Linux | One workspace instead of separate panels and SSH tools |
| Dev teams outgrowing PaaS tools | Projects, services, compose stacks and environment variables on your own servers |
| Agencies | Per-client tenants, access without sharing passwords |
| Regulated or offline sites | Self-hosted, self-contained packages, signed updates |

## What Forge does

- **Clusters, nodes and environments.** Group machines into clusters and
  separate projects into environments. See [Add nodes](/docs/guides/add-nodes/).
- **Identity and access.** Local users with TOTP or passkeys, OIDC single
  sign-on, API keys, and roles per workspace, cluster or environment enforced
  by [Cedar](https://www.cedarpolicy.com). See
  [Roles and access](/docs/guides/roles-and-access/).
- **Agents.** Enrollment with a one-time token, mTLS over HTTP/2, signed and
  replay-protected commands, and verified self-updates on Linux. See
  [Agents](/docs/concepts/agents/).
- **Browser terminal.** SSH with short-lived certificates from Forge's SSH
  CA. See [Browser terminal](/docs/guides/terminal/).
- **Software.** An App Store for runtimes, web servers and databases;
  services with environment variables; Docker containers and compose
  stacks. See [App Store](/docs/guides/app-store/) and
  [Deploy a compose stack](/docs/guides/compose-stacks/).
- **Files.** A file explorer and temporary SFTP access. See
  [Browse files on a node](/docs/guides/browse-files/).
- **Network.** Nginx edge sites, cluster ingress, PowerDNS records and port
  tunnels to services over the API port. See [Sites and DNS](/docs/guides/sites-and-dns/)
  and [Port tunnels](/docs/concepts/port-tunnels/).
- **Durable jobs and audit.** Every operation is a job that survives a
  restart; every mutation lands in a hash-chained, signed audit log. See
  [Jobs](/docs/concepts/jobs/) and [Audit](/docs/concepts/audit/).
- **Automation.** The `forge` CLI, the REST API with
  `GET /api/openapi.json`, and an MCP endpoint for AI agents. See
  [Connect AI agents](/docs/guides/connect-ai-agents/).
- **Alerts.** Rules on node and health evidence open alarms; policies send
  them to webhook and Slack-compatible channels. See
  [Alerts](/docs/concepts/alerts/).
- **Plugins.** Signed WebAssembly components with a typed host API, budgets
  and quarantine. See [Writing plugins](/docs/guides/writing-plugins/).

Browser RDP, GitHub deployments, backups and database management are not
available yet. See the [roadmap](/docs/roadmap/).
