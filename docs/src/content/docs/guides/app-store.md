---
title: App Store
description: Install runtimes, databases, web servers and platform plugins on one or many nodes.
sidebar:
  order: 4
---

This guide shows you how to install a package from the App Store on one or
many nodes, manage it and keep it updated. Forge installs from the operating
system's package manager and starts one durable job per node.

## Before you begin

- Installing, removing, starting, stopping and updating packages is
  `package.manage` on the node (**Operator**, **Admin** or
  **Platform admin**), with an MFA-verified session. Every action is audited.
- Browsing the catalog and the installed packages needs `view`.
- Check that your nodes run a supported OS (see
  [Where packages come from](#where-packages-come-from)).

## Install a package

1. Open **Packages → App Store** and select a package's **Details** (or its
   name) to read what it does and what it needs.
2. Select **Install**.
3. Set the version and options, choose the nodes, review, and start the
   install. Licences that need acceptance (the SQL Server EULA) must be
   ticked explicitly.
4. Follow the install: each node gets a tab with its live install log.

**Packages → Installed**, and the **Packages** tab of each node, show what
runs where, with start, stop, restart, status and uninstall.

## Catalog

| Category | Packages |
| --- | --- |
| Runtimes | PHP-FPM 7.4–8.4 (side by side, extension picker), Python 3, Node.js 20/22/24, Java (Temurin 17/21/25), .NET 8/9/10, Go |
| Web | Nginx, Apache HTTP Server, HAProxy, Caddy, Tomcat |
| Databases | PostgreSQL 15–18, MySQL 8.4/8.0, MariaDB, Redis, Valkey, Memcached, MongoDB 7/8, ClickHouse, InfluxDB 2, Elasticsearch 8/9, OpenSearch 2/3, SQL Server (container) |
| Messaging | RabbitMQ (with management UI), Mosquitto (MQTT) |
| Monitoring | Grafana, Loki, Prometheus, node exporter, Zabbix agent 2 |
| DevOps | Jenkins LTS, GitLab CE |
| Storage | RustFS (S3-compatible, container). To run RustFS as a service with a services file instead, see [Run RustFS as a service](/docs/guides/run-rustfs-service/) |
| Containers | Docker Engine |
| DNS | PowerDNS (authoritative, HTTP API) |
| Tools | Certbot (Let's Encrypt for Ingress, renewals reload Nginx), Keepalived (virtual IP for edges on one LAN) |

More packages and integrations are planned; see the [roadmap](/docs/roadmap/).

### Where packages come from

Forge installs from the operating system's package manager, so packages run
as the distribution's services and can still be updated by hand on the node:

| Node | Package manager | Supported |
| --- | --- | --- |
| Debian 12, Ubuntu 22.04/24.04 | apt | every package |
| RHEL, Rocky, Alma 9 | dnf (EPEL, Remi, PGDG or the vendor's repository where needed) | every package except where noted on its card (Prometheus depends on EPEL) |
| Windows 11, Windows Server 2025 | winget | PostgreSQL, Node.js, Java (Eclipse Temurin), .NET |

Vendor repositories are added with their signing keys (for example PGDG for
PostgreSQL, NodeSource, Adoptium, MongoDB, Docker, Grafana, Elastic,
OpenSearch, ClickHouse, InfluxData, Team RabbitMQ, Jenkins, GitLab, Zabbix).
On RHEL, Forge also opens the package's port in firewalld when it is bound
to all interfaces, and sets the SELinux port label or boolean it needs (for
example `httpd_can_network_connect` for Nginx). On Debian and Ubuntu it opens
the port in ufw only if ufw is active.

Before a first install, Forge checks that the package's port is free and stops
with the name of the program using it.

Go and RustFS have no package repository: reinstall a newer version to update.
SQL Server and RustFS run as containers and need *Docker Engine* on the same
node; Forge refuses to start them otherwise. Deploying a container service
is different: it installs Docker Engine on a node that has none (see
[`forge services deploy`](/docs/reference/cli/services/#forge-services-deploy)). Docker is a package too: nothing
is installed by default.

On Windows, winget must be present (it ships with Windows 11 and Windows
Server 2025; install App Installer on Server 2022). The agent service runs the
install itself, keeps generated secrets in `C:\ProgramData\OrbynForge\pkgs`
(readable by SYSTEM and Administrators only) and installs exactly the build
shown, like on Linux.

## Example: a three-node layout

1. *node-1* — install **PostgreSQL** (or SQL Server after Docker) and **Redis**.
2. *node-2* and *node-3* — install **Docker Engine**, or a runtime such as
   **Python** / **.NET** for apps run as system services.
3. Two edge nodes — **Nginx** plus **Keepalived** (one MASTER, one BACKUP).

## Where things go

Every node uses the same layout:

| Path | Contents |
| --- | --- |
| `/opt/forge/pkgs/<package>/<version>/` | Files a package brings itself (.NET, Go…) |
| `/var/lib/forge/pkgs/<package>/` | Package data |
| `/etc/forge/pkgs/<package>.env` | `FORGE_PKG_<PACKAGE>_*` variables (mode 0600) |
| `/srv/forge/apps/<environment>/<app>/` | Your services (see [Services](/docs/guides/services/)) |

For example PostgreSQL writes `FORGE_PKG_POSTGRESQL_HOST`, `_PORT`, `_USER`,
`_PASSWORD` and `_URL`. Services can load the file as an `EnvironmentFile`.

## Security

- No scripts over the wire. Recipes are compiled into the node's
  `forge-agent-helper`. Forge sends only *package, version, action* and typed
  parameters (ports, fixed choices, identifier lists, addresses), which the
  helper validates again before running anything.
- Root only through the helper. The agent stays unprivileged; the helper
  runs as root and serves a socket only root and the `forge-agent` user can use.
- Passwords stay on the node. Database passwords, API keys and tokens are
  generated on the node and kept in its env file. The console shows the
  variable names, with secret values redacted.
- Installing, removing, starting and stopping is `package.manage` and needs
  an MFA-verified session; every action is audited.

## Package pages

Select a package's name, or **Details**, for its own page:

- **How Forge uses it:** its role, and the Forge features built on it (for example Nginx → Ingress, PowerDNS → the DNS page, runtimes → service dependencies).
- **Using it:** the steps from install to use in a service.
- **Configuration:** every install setting, with its type, default and effect, plus the packages it needs first.
- **What it gives services:** the `FORGE_PKG_*` variables in `/etc/forge/pkgs/<id>.env`. Generated passwords and keys are marked *secret*: they are created on the node and stay there.
- **On the node:** services and paths.
- **Installed on:** every node that has it, with status and actions (refresh, start or stop, restart, uninstall).

**Install** opens the installer. Each node gets a tab with its live install log.

## Update a package

Packages installed from a package repository (apt, dnf or winget) are
tracked on **Packages → Updates**. Packages that move together (for example
`postgresql-17` and `postgresql-client-17`, or every `php8.3-*` package) are
updated to the same build in one step. Each row shows:

- the installed build (for example `17.6-1.pgdg24.04+1`) and the version
  the running service reports;
- the available build that the node's own repositories offer, and the
  size of the update: *security / packaging* (same version, new package
  revision), *patch*, *minor* or *major*;
- the vendor's release line: LTS, latest, and how long until end of
  life. When the vendor has released a newer build that your node's
  repositories do not have yet, the row says *released upstream · not on
  this node yet*.

Forge asks every node every six hours. Use **Check now** to ask straight away.
Vendor release data comes from [endoflife.date](https://endoflife.date) and
is refreshed daily.

**Update** shows what will happen first. The service restarts, so expect a
short outage, and the dialog lists the services on that node that use the
package. Forge then installs exactly the build shown, keeps your
configuration files, and checks that the service reports the new version. If
it does not, the update is marked failed and tells you to restart the service.

Because packages come from the distribution's package manager, you can still
update by hand on the node (`apt-get upgrade`). Forge picks up the new build
on its next check.

**Update policy** per installation:

| Policy | What happens |
| --- | --- |
| **Notify me** (default) | Updates are listed; you apply them. |
| **Auto-apply patches** | Security, packaging and patch updates are applied on the next six-hourly check. Minor and major updates still wait for you. |

Packages installed from a vendor tarball or script (Go, RustFS) are not
tracked; update them by reinstalling a newer version.

## Verify

- **Packages → Installed** lists the package on each node with status
  `running` (or installed, for packages without a service).
- The node's **Packages** tab shows the same.
- For packages that give services variables,
  `/etc/forge/pkgs/<package>.env` exists on the node.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| The install stops with the name of a program | The package's port is already in use | Stop or move that program, or choose another port |
| SQL Server or RustFS refuses to start | **Docker Engine** is not on the node | Install **Docker Engine** first |
| A package is not offered for a Windows node | Only PostgreSQL, Node.js, Java and .NET install through winget | Use a Linux node |
| winget is missing on Windows Server 2022 | winget does not ship with it | Install App Installer |
| An update is marked failed | The service did not report the new version | Restart the service on the node, then **Check now** |
| A Prometheus install fails on RHEL | It depends on EPEL | Enable EPEL on the node |
| Go or RustFS shows no updates | They have no package repository | Reinstall a newer version |

## Next steps

- [Services](/docs/guides/services/) to use runtimes and package
  variables.
- [Run RustFS as a service](/docs/guides/run-rustfs-service/).
- [Containers](/docs/guides/containers/)
