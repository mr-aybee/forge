---
title: Server installer
description: Every option, environment variable, file, service, message and exit code of the control plane installers install.sh (Linux) and install.ps1 (Windows).
sidebar:
  order: 93
---

This page lists exactly what the control plane installers accept, what they
create and what they print. Each release package carries one installer:
`install.sh` in the Linux packages and `install.ps1` in the Windows package.
For a walk-through, see [Installation](/docs/getting-started/installation/).

:::note
Releases do not publish a Windows package (`forge-<version>-x86_64-windows.zip`)
yet. `install.ps1` and `get-forge.ps1` still exist and are described here, but
`get-forge.ps1` stops with `The release has no Windows package.` against a
current release. A Windows control plane comes only from a package you build
from source with `scripts/packaging/package.sh`, and it is not verified.
:::

| Installer | Platform | Runs as | Supervisor it sets up |
| --- | --- | --- | --- |
| `install.sh` | Linux x86_64 or aarch64 with systemd | root | `forge-server.service` (systemd) |
| `install.ps1` | Windows Server 2019, 2022, 2025; Windows 10, 11 | Administrator | Scheduled task **Orbyn Forge** (SYSTEM) |

Both installers install and upgrade. Run the installer from a newer package
to upgrade: it replaces the binaries and keeps the configuration and the data.

## install.sh (Linux)

Installs or upgrades the control plane from the extracted Linux package.

**Requires:** root, systemd (`/run/systemd/system` must exist), `curl` (for
the health check), and the extracted package as the working copy
(`forge-server` next to `install.sh`).

### Synopsis

```
sudo ./install.sh [--public-url URL] [--public-address URL] [--trusted-proxies LIST]
                  [--admin-username NAME] [--admin-email EMAIL]
                  [--agent-host NAME] [--prefix DIR] [--environment ENV]
                  [--mfa required|off] [--dev-skip-mfa]
                  [--allow-elevated-tokens yes|no]
                  [--install-agent [--cluster NAME] [--terminal-account NAME]]
                  [--db sqlite|postgres] [--db-url URL | --db-local] [--db-name NAME]
                  [--allow-shared-db]
                  [--analytics duckdb|chdb|clickhouse|off]
                  [--analytics-url URL] [--analytics-name NAME]
                  [--yes | --non-interactive]
sudo ./install.sh                # on a terminal: asks the questions below
./install.sh --help
```

### Interactive mode

The installer asks questions only when all of these are true:

- standard input and standard output are both terminals,
- you did not pass `--yes` or `--non-interactive`,
- you passed no option that chooses a setting (every option except
  `--prefix`, `--yes` and `--help`).

Otherwise it runs from the options and defaults alone, exactly as in
scripts, CI and `curl … | sh`. Each question shows its default in brackets;
press Enter to take it.

| # | Question | Default | Asked when | Same as |
| --- | --- | --- | --- | --- |
| 1 | Public URL users open | `http://<first IP of hostname -I>:3880` | No `forge.toml` yet | `--public-url` |
| 2 | Environment | `development` | Always | `--environment` |
| 3 | Administrator user name | `admin` | No database yet | `--admin-username` |
| 4 | Administrator password (hidden, typed twice; Enter generates one) | generate | No database yet | `FORGE_ADMIN_PASSWORD` |
| 5 | Allow elevated API tokens for CI (no MFA prompt)? | `n` for `production`, `y` otherwise | `--mfa required` is given | `--allow-elevated-tokens` |
| 6 | Also run workloads on this server (install the Forge agent here)? | `N` | Always | `--install-agent` |
| 6a | Cluster for this server (created if missing) | `default` | Answer 6 is yes | `--cluster` |
| 6b | Terminal account on this server | `forge-ops` | Answer 6 is yes | `--terminal-account` |

The installer does not ask about MFA. A new install leaves MFA to each user
(`require_mfa = false`); pass `--mfa required` to require it for guarded
actions.

The password is read with terminal echo off. The installer restores the
terminal when it exits, also on Ctrl+C. A **REVIEW** screen then lists the
answers and asks `Install with these settings? [Y/n]`. Answer `n` to exit
with `0` without changing anything. The numbers on screen run 1, 2, 3 … in
the order the questions are asked, so they can differ from this table.

An example session on a new host:

```
  ╭─ Orbyn Forge · control plane installer
  │  New installation  ·  version <version>  ·  host <fqdn>
  ╰────────────────────────────────────────────────────────────

  Answer each question, or press Enter for the default in brackets.

  1. Public URL users open [http://<server-ip>:3880]
  2. Environment (production, staging, development, test) [development]
  3. Administrator user name [admin]
  4. Administrator password (hidden; Enter generates one) [generate]
     Password:
  5. Also run workloads on this server (install the Forge agent here)? [y/N] y
  6. Cluster for this server (created if missing) [default] lab
  7. Terminal account on this server [forge-ops]

  REVIEW
    Public URL    http://<server-ip>:3880
    Environment   development
    Admin         admin
    Password      generated, shown once at the end
    MFA           each user's choice (sign-in only)
    Agent         yes: cluster lab, terminal account forge-ops

  8. Install with these settings? [Y/n]
```

### Options

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--public-url` | URL | `http://<fqdn>:3880`, where `<fqdn>` is `hostname -f` (or `hostname`) | No | URL users open. Written to `public_url`. An `https://` URL also sets `cookie_secure = true`. First install only: an existing `forge.toml` is kept | `--public-url https://<forge-host>` |
| `--admin-username` | string | `admin` | No | Sign-in name of the first platform administrator. First install only | `--admin-username <admin-name>` |
| `--admin-email` | string | none | No | Email of the first administrator. You can add it later in the console. First install only | `--admin-email <admin-email>` |
| `--public-address` | URL (origin) | none | No | Address people use when Forge is published on a domain through an ingress, for example `https://forge.example.com`. Stored as the runtime [public address](/docs/reference/configuration/#public-address) with `forge-server public-address set`, on a first install and on an upgrade; links in alerts use it, and `--public-url` stays the direct address. Refused (the installer stops) when it has a path, query or user name | `--public-address https://forge.example.com` |
| `--trusted-proxies` | comma-separated IPs or CIDRs | none (`[]`) | No | Reverse proxies, such as the ingress edge nodes, whose `X-Forwarded-For` and `X-Forwarded-Proto` Forge trusts. Written to `[security] trusted_proxies`. First install only: edit `forge.toml` on an existing install | `--trusted-proxies 192.0.2.20,192.0.2.21` |
| `--agent-host` | host name or IP | the host part of `--public-url` | No | Name or IP that nodes use to reach this server. Sets `agent_public_url = "https://<agent-host>:3843"` and is the first entry of `agent_hostnames`. First install only | `--agent-host <forge-ip>` |
| `--prefix` | directory | `/opt/forge` | No | Where the binaries and agent binaries go. Also where the installer looks for an existing version. Use the same value on every run | `--prefix /srv/forge` |
| `--environment` | `production`, `staging`, `development` or `test` | not written, so the server uses `production` | No | Sets the top-level `environment` key in `forge.toml`. Applied on every run where you pass it, including upgrades | `--environment staging` |
| `--mfa` | `required` or `off` (`on` is accepted as `required`) | `off` on a new install (MFA is each user's choice for their own sign-in); an upgrade keeps the current setting | No | `required` also makes guarded actions need MFA. Sets `[security] require_mfa` with the same editor as `forge-server mfa on\|off`. Applied on every run where you pass it | `--mfa off` |
| `--dev-skip-mfa` | flag | off | No | **Deprecated.** Same as `--mfa off` | `--dev-skip-mfa` |
| `--allow-elevated-tokens` | `yes` or `no` (`y`, `n`, `true`, `false`, `on`, `off` also work) | on a new `forge.toml`: `no` when the environment is `production` (or not set) and `--mfa required` is given, otherwise `yes`. An upgrade keeps the current setting | No | Writes `[security] allow_elevated_tokens = true\|false`. Elevated API tokens (for CI) skip the MFA prompt on guarded actions when this is `true`. On an existing `forge.toml` the installer changes the line in place, adds it under `[security]`, or appends a `[security]` table | `--allow-elevated-tokens no` |
| `--install-agent` | flag | off | No | After the health check, also installs the Forge agent on this server and enrolls it, so the server runs workloads too. See [Agent on the server](#agent-on-the-server). A failure here does not fail the install | `--install-agent` |
| `--cluster` | cluster name (`A-Z`, `a-z`, `0-9`, `.`, `_`, `-`) | `default` | No | Cluster the server's agent joins. The cluster is created if it does not exist. Used with `--install-agent` | `--cluster lab` |
| `--terminal-account` | account name (`a-z`, `0-9`, `_`, `-`) | `forge-ops` | No | Local account the agent uses for browser terminals on this server. Used with `--install-agent` | `--terminal-account forge-ops` |
| `--db` | `sqlite` or `postgres` | `sqlite` | No | Operational database. First install only. See [Choose and connect databases](/docs/guides/databases/) | `--db postgres` |
| `--db-url` | URL without database name and password | none | With `--db postgres`, unless `--db-local` | PostgreSQL server, e.g. `postgres://forge@db1:5432`. Implies `--db postgres`. The password comes from `FORGE_DATABASE_PASSWORD` and is stored in `/etc/forge/db-password` (mode `0600`). A URL with a password is refused | `--db-url postgres://forge@db1:5432` |
| `--db-local` | flag | off | No | Installs the PostgreSQL server from the system packages (`apt-get` or `dnf`; on RHEL 8 it enables the `postgresql:15` module stream first). On RHEL-likes it also initialises the cluster and switches local TCP logins in `pg_hba.conf` from `ident` to `scram-sha-256`. It then creates a login role `forge` with `CREATEDB` and a generated password, and uses it. Implies `--db postgres` | `--db-local` |
| `--db-name` | name (`A-Z`, `a-z`, `0-9`, `_`) | `forge_op` | No | Database name. Created when missing, reused when it exists; never dropped or recreated | `--db-name forge_op` |
| `--allow-shared-db` | flag | off | No | Uses a database that already holds tables that are not Forge's | `--allow-shared-db` |
| `--analytics` | `duckdb`, `chdb` (also accepted as `embedded`), `clickhouse` or `off` | `duckdb` | No | Analytics database for node history. `chdb` needs the engine binary (`forge-server analytics install`); `duckdb` runs inside the analytics worker and needs nothing else | `--analytics clickhouse` |
| `--analytics-url` | URL without password | none | With `--analytics clickhouse` | ClickHouse HTTP(S) address, e.g. `https://default@ch1:8443`. The password comes from `FORGE_ANALYTICS_PASSWORD` and is stored in `/etc/forge/analytics-password` | `--analytics-url https://default@ch1:8443` |
| `--analytics-name` | name (`A-Z`, `a-z`, `0-9`, `_`) | `<db-name>_analytics` | No | Analytics database name | `--analytics-name forge_op_analytics` |
| `--yes`, `--non-interactive` | flag | off | No | Never ask questions, even on a terminal. Use it in scripts that run under a terminal (for example `ssh -t`) | `--yes` |
| `--uninstall` | flag | off | No | Stops and disables `forge-server.service`, removes the unit and the prefix, and keeps `/etc/forge` and `/var/lib/forge`. Asks for confirmation unless `--yes`. Run it from `/etc/forge/install.sh` or a package; pass `--prefix` if you installed with one. Firewall rules and an agent on the same server are left in place | `--uninstall --yes` |
| `--purge` | flag | off | With `--uninstall` | Also deletes `/etc/forge`, `/var/lib/forge` (database, keys, CA, backups) and the `forge` user | `--uninstall --purge --yes` |
| `-h`, `--help` | flag | | No | Prints the usage comment at the top of the script and exits with `0` | `--help` |

Every option except the flags takes a value as the next argument. The
`--option=value` form is not accepted.

### Environment variables

| Name | Default | Description |
| --- | --- | --- |
| `FORGE_ADMIN_PASSWORD` | unset: the installer generates a password | Password of the first administrator, at least 12 characters. When it is set, nothing is saved to disk. Read only when the installer creates the administrator |
| `NO_COLOR` | unset | Any value turns off coloured output. Colour is also off when standard output is not a terminal |

The installer uses box-drawing and tick characters (`✔`, `▲`, `╭`) only when
`locale charmap` reports `UTF-8`. Otherwise it prints `+`, `!` and `-`.

### Examples

Install with the defaults (console on port 3880 of this host):

```sh
sudo ./install.sh
```

Install behind a TLS proxy, with a named administrator:

```sh
sudo ./install.sh --public-url https://<forge-host> --admin-username <admin-name> --admin-email <admin-email>
```

Choose the administrator password yourself, without putting it on a command
line or in shell history:

```sh
sudo -s
read -rs FORGE_ADMIN_PASSWORD && export FORGE_ADMIN_PASSWORD   # type the password, then press Enter
./install.sh --public-url https://<forge-host>
unset FORGE_ADMIN_PASSWORD
exit
```

Install a development server without questions that also runs workloads,
with MFA off:

```sh
sudo ./install.sh --yes --public-url http://<server-ip>:3880 --environment development \
  --mfa off --allow-elevated-tokens yes --install-agent --cluster <cluster-name>
```

Upgrade from a newer package and turn MFA off on a lab server:

```sh
cd forge-<version>-x86_64-linux
sudo ./install.sh --environment development --mfa off
```

### What it does

The installer runs these steps in order. Each prints one line (see
[What it prints](#what-installsh-prints)).

Before the first line, and before it changes anything, the installer checks
the package: the package folder's architecture (`forge-<version>-<arch>-linux`)
matches `uname -m`; `forge-server`, `forge-plugin-runner`,
`forge-analytics-worker`, `dist/SHA256SUMS` and `dist/MANIFEST.json` exist;
every agent binary in `dist/` matches `dist/SHA256SUMS`; `forge-server` and
`forge-analytics-worker` run (`--version`); and at least 500 MB are free
next to the prefix. Any failure stops the installer with a message that says
what to download again. The signature of the package itself is checked by
`get-forge.sh` before it runs the installer.

| Line | What happens |
| --- | --- |
| **Service account** | Creates the system user `forge` (home `/var/lib/forge`, shell `/usr/sbin/nologin`) unless it exists |
| **Directories** | Creates the prefix, `<prefix>/dist` and `/etc/forge` (`0755`), and `/var/lib/forge` (`0700`, owner `forge:forge`) |
| **Binaries** | Copies `forge-server`, `forge-plugin-runner` and `forge-analytics-worker` to the prefix (`0755`) by writing `<name>.new` and renaming it, so a running server keeps its file until it restarts. Copies every file in the package's `dist/` to `<prefix>/dist`. Gives the whole prefix to `forge:forge`, because the self-updater replaces these files as the service user. Copies the installer to `/etc/forge/install.sh` (`0700`, root) for `--uninstall`. On SELinux systems, runs `restorecon -RF` on the prefix, `/etc/forge` and `/var/lib/forge`. The line counts the `forge-agent*` files |
| **Configuration** | Writes `/etc/forge/forge.toml` if it does not exist (see [The forge.toml it writes](#the-forgetoml-it-writes)), mode `0640`, group `forge`. An existing file is kept unchanged |
| **Environment** | Only with `--environment`: removes any `environment = ` line and inserts the new one after line 1 |
| **Elevated tokens** | With `--allow-elevated-tokens` on an existing `forge.toml`, sets `allow_elevated_tokens` in `[security]`. Then reports the value in the file |
| **MFA** | With `--mfa`, runs `forge-server --config /etc/forge/forge.toml mfa on\|off`. Then reports the result of `mfa status` |
| **Administrator** | Creates the first platform administrator when `/var/lib/forge/forge.db` does not exist and `forge.toml` has no `database_url` line (see [The first administrator](#the-first-administrator)) |
| **Service** | Writes `/etc/systemd/system/forge-server.service` (every run), restores its SELinux label, then runs `systemctl daemon-reload`, `enable` and `restart`. `forge-server` starts and supervises `forge-analytics-worker` and `forge-plugin-runner` itself; they have no units of their own |
| **Firewall** | Opens the ports in `forge.toml` permanently in firewalld when it is running, in ufw when it is active, or else in plain iptables when `INPUT` ends in a `REJECT`/`DROP` rule or has a `DROP` policy (the rules go above it, for IPv4 and IPv6, and are saved with `netfilter-persistent`, `/etc/iptables/rules.v4` or `/etc/sysconfig/iptables`): the port of `api_listen` (default `3880/tcp`), of `agent_listen` (`3843/tcp`) and of `[analytics] worker_agent_listen` (`3832/tcp`, the analytics worker's upload port). Otherwise changes nothing. A re-run adds no duplicate rules. It never changes a cloud provider's firewall. With another firewall, see [Open the analytics upload port](#open-the-analytics-upload-port) |
| **Health check** | Polls `/api/health` on the `api_listen` port (on `127.0.0.1` when it listens on all addresses) once a second, up to 60 attempts |
| **Analytics worker** | Unless `[analytics] engine = "off"`: waits up to 30 s for the worker's upload port to accept connections. A worker that does not come up is a warning, not a failure: the console works without it |
| **Agent** | Only with `--install-agent`: enrolls this server as a node (see [Agent on the server](#agent-on-the-server)) |

#### Open the analytics upload port

With analytics on, nodes upload their spools to the analytics worker on
`3832/tcp` ([Network ports](/docs/reference/network-ports/)). The
installer opens it in firewalld, ufw or plain iptables. With another firewall, or a
network firewall between the nodes and the server, open it from the
nodes' networks:

```sh
# firewalld
sudo firewall-cmd --permanent --add-port=3832/tcp && sudo firewall-cmd --reload
# ufw
sudo ufw allow 3832/tcp
# plain iptables: above the first REJECT/DROP rule (here number 5), then save
sudo iptables -I INPUT 5 -p tcp -m state --state NEW --dport 3832 -j ACCEPT && sudo netfilter-persistent save
```

### Agent on the server

With `--install-agent` (or answer yes to question 7), the installer runs,
after the health check:

1. `forge-server --config /etc/forge/forge.toml enroll-token --cluster
   <cluster> --hostname "$(hostname)" --ttl-minutes 15` as the `forge` user.
   It creates the cluster if it is missing and prints a one-time enrollment
   token that expires after 15 minutes.
2. `curl -fsSL "http://127.0.0.1:3880/install.sh?terminal_account=<account>" | sh`
   with the token in the `FORGE_ENROLL_TOKEN` environment variable. This is
   the same node installer that other nodes run.

The token is never on a command line or in a log, and the installer unsets
it afterwards. The node installer's output is kept back; when a step fails,
the installer prints a `▲ Agent` line, the last 15 lines of that output and,
in the summary, the commands to retry by hand. The server install still
exits with `0`.

:::note
The agent on the server is an ordinary node. It is authorized and audited
like any other node, and you can remove it from **Nodes** in the console.
:::

### Files and services it creates

| Path | Owner | Mode | Contents |
| --- | --- | --- | --- |
| `/opt/forge/forge-server` | `forge:forge` | `0755` | Control plane |
| `/opt/forge/forge-plugin-runner` | `forge:forge` | `0755` | Plugin runner |
| `/opt/forge/forge-analytics-worker` | `forge:forge` | `0755` | Analytics worker, started by `forge-server` |
| `/opt/forge/dist/` | `forge:forge` | files as in the package; `forge-agent*` `0755` | Agent binaries handed to nodes, and `SHA256SUMS` |
| `/etc/forge/forge.toml` | `root:forge` | `0640` | Configuration (first install only) |
| `/var/lib/forge/` | `forge:forge` | `0750` | Database, keys, internal CA, backups |
| `/var/lib/forge/initial-admin-password` | `root:root` | `0600` | Generated administrator login. Forge deletes it when the administrator changes the password |
| `/etc/systemd/system/forge-server.service` | `root:root` | default | systemd unit (rewritten every run) |

The paths under `/opt/forge` move with `--prefix`. `/etc/forge` and
`/var/lib/forge` are fixed.

The unit it writes (with the default prefix):

```ini
[Unit]
Description=Orbyn Forge control plane
After=network-online.target
Wants=network-online.target

[Service]
User=forge
Group=forge
ExecStart=/opt/forge/forge-server --config /etc/forge/forge.toml serve
Restart=always
RestartSec=3
LimitNOFILE=65536
UMask=0077
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=true
ReadWritePaths=/var/lib/forge /opt/forge

[Install]
WantedBy=multi-user.target
```

### The forge.toml it writes

On a first install, with `--public-url https://<forge-host>` on a host whose
FQDN is `<fqdn>`:

```toml
# Orbyn Forge control plane (written by install.sh; see forge.toml.example).
data_dir = "/var/lib/forge"
api_listen = "0.0.0.0:3880"
public_url = "https://<forge-host>"
cookie_secure = true
agent_listen = "0.0.0.0:3843"
agent_public_url = "https://<forge-host>:3843"
agent_hostnames = ["<forge-host>", "<fqdn>", "localhost", "127.0.0.1"]
agent_dist_dir = "/opt/forge/dist"

[update]
enabled = true

[security]
# MFA is each user's choice and protects their sign-in. true also requires it
# for guarded actions. Change with: forge-server --config /etc/forge/forge.toml mfa on|off
require_mfa = false
# Elevated API tokens (for CI) skip the MFA prompt when true.
allow_elevated_tokens = false
```

`allow_elevated_tokens` follows `--allow-elevated-tokens` or its default.
`cookie_secure` is `false` when the public URL starts with `http://`. See
[Configuration](/docs/reference/configuration/) for every key.

:::note
`api_listen = "0.0.0.0:3880"` serves plain HTTP on every interface. In
production, bind it to `127.0.0.1:3880` and put a TLS proxy in front. See
[Run Forge in production](/docs/guides/production-setup/#1-put-a-tls-proxy-in-front-of-the-console).
:::

### The first administrator

The installer creates the administrator only when there is no
`/var/lib/forge/forge.db` and `forge.toml` has no `database_url` line. With
PostgreSQL (`database_url` set in a `forge.toml` you wrote before the first
run), it creates no administrator: run `forge-server init` yourself.

- **`FORGE_ADMIN_PASSWORD` unset:** the installer generates 24 characters
  from an alphabet without `0`, `O`, `1`, `l` and `I`, prints them once in
  the summary and saves them, with the username, to
  `/var/lib/forge/initial-admin-password` (`root:root`, `0600`).
- **`FORGE_ADMIN_PASSWORD` set:** it must have at least 12 characters.
  Nothing is saved.

It then runs `forge-server --config /etc/forge/forge.toml init --username
<name> [--email <email>] [--generated-password]` as the `forge` user, with the
password in the environment. If `init` fails, the installer deletes
`forge.db`, `forge.db-wal`, `forge.db-shm` and the password file, prints the
`init` output indented, and exits with `1`. Fix the cause and run the
installer again.

### Fresh install and upgrade

The installer treats the run as an upgrade when `<prefix>/forge-server`
exists. It reads both versions with `forge-server --version`.

| | New installation | Upgrade |
| --- | --- | --- |
| Banner | `New installation` | `Upgrade <old> → <new>` |
| Binaries and agent binaries | Installed | Replaced |
| `forge.toml` | Written | Kept. `--public-url`, `--agent-host` and the `[security]` default have no effect |
| `--environment`, `--mfa`, `--allow-elevated-tokens` | Applied | Applied when passed; otherwise the file is unchanged |
| `--install-agent` | Enrolls the server | Skipped when `forge-agent` is already running on the host, so a re-run never adds a second node record |
| Administrator | Created | `existing accounts kept` |
| Unit file | Written | Rewritten, then the service restarts |
| Summary | `is installed and running`, with an **ADMINISTRATOR** section | `upgraded to <new> and running`, no **ADMINISTRATOR** section |

The summary always reports `public_url` and `agent_public_url` as they are
in `forge.toml`, not the options you passed.

### What install.sh prints

A new installation on a UTF-8 terminal, with no host firewall active:

```
  ╭─ Orbyn Forge · control plane installer
  │  New installation  ·  version <version>  ·  host <fqdn>
  ╰────────────────────────────────────────────────────────────

  ✔ Service account        forge (system user, no login)
  ✔ Directories            /opt/forge · /etc/forge · /var/lib/forge
  ✔ Binaries               server, analytics worker, plugin runner, 5 agent binaries
  ✔ Configuration          /etc/forge/forge.toml (new)
  ✔ Elevated tokens        not allowed
  ✔ MFA                    required for guarded actions
  ✔ Administrator          admin (platform admin)
  ✔ Service                forge-server.service (enabled, restarts on failure; starts the analytics worker)
  ✔ Firewall               no active firewalld, ufw or blocking iptables rules; allow 3880/tcp (console), 3843/tcp (agents), 3832/tcp (analytics uploads) in yours
  ▲ Cloud firewall         also allow TCP 3880, 3843 and 3832 in your provider's firewall (security group, security list); the installer cannot change it
  ✔ Health check           responding after 2s
  ✔ Analytics worker       duckdb, uploads on port 3832

  Orbyn Forge <version> is installed and running.

  ACCESS
    Console       http://<fqdn>:3880
    Agents        https://<fqdn>:3843  (nodes connect out to TCP 3843 and 3832)
    Analytics     duckdb, uploads on port 3832

  ADMINISTRATOR
    Username      admin
    Password      <generated-password>
    Saved to      /var/lib/forge/initial-admin-password (root only, deleted when you change it)
    · Sign in, then in Profile add your email, change the password and set up MFA.

  SECURITY
    MFA           each user's choice (forge-server mfa on|off)
    Elevated      API tokens allowed ([security] allow_elevated_tokens)

  AGENT
    This server   not installed on this server

  INSTALLED
    Binaries      /opt/forge
    Agents        /opt/forge/dist
    Config        /etc/forge/forge.toml
    Data          /var/lib/forge (database, keys, internal CA, backups)
    Service       forge-server.service (systemd, user forge)

  OPERATE
    Status        systemctl status forge-server
    Logs          journalctl -u forge-server -f
    Uninstall     sudo sh /etc/forge/install.sh --uninstall

  NEXT STEPS
    1. Open http://<fqdn>:3880 and sign in as admin.
       The password is also in /var/lib/forge/initial-admin-password (sudo cat it).
    2. Allow TCP 3843 and 3832 from your nodes to this server.
    3. Add nodes: Overview > Add node (docs: /docs/guides/add-nodes/).

  ▲ Production             Put a TLS proxy (Caddy, Nginx) in front and use an https:// public URL.

```

An upgrade with `--environment development --mfa off`, on a host with ufw
active:

```
  ╭─ Orbyn Forge · control plane installer
  │  Upgrade <old-version> → <version>  ·  version <version>  ·  host <fqdn>
  ╰────────────────────────────────────────────────────────────

  ✔ Service account        forge (system user, no login)
  ✔ Directories            /opt/forge · /etc/forge · /var/lib/forge
  ✔ Binaries               server, analytics worker, plugin runner, 5 agent binaries
  ✔ Configuration          /etc/forge/forge.toml (kept)
  ✔ Environment            development
  ✔ Elevated tokens        not allowed
  ✔ MFA                    each user's choice (protects sign-in; forge-server mfa on also guards actions)
  ✔ Administrator          existing accounts kept
  ✔ Service                forge-server.service (enabled, restarts on failure; starts the analytics worker)
  ✔ Firewall               opened 3880/tcp (console), 3843/tcp (agents), 3832/tcp (analytics uploads) (ufw)
  ✔ Health check           responding after 1s
  ✔ Analytics worker       duckdb, uploads on port 3832

  Orbyn Forge upgraded to <version> and running.

  ACCESS
    Console       http://<fqdn>:3880
    Agents        https://<fqdn>:3843  (nodes connect out to TCP 3843 and 3832)

  SECURITY
    MFA           each user's choice (forge-server mfa on|off)
    Elevated      API tokens not allowed ([security] allow_elevated_tokens)

  AGENT
    This server   not installed on this server

  INSTALLED
    ...
```

Other lines you can see:

| Line | When |
| --- | --- |
| `Administrator          <name> <<email>> (platform admin)` | `--admin-email` was given |
| `Password      the value of FORGE_ADMIN_PASSWORD` | `FORGE_ADMIN_PASSWORD` was set (replaces the **Password** and **Saved to** rows) |
| `Email         <email>` | `--admin-email` was given (in **ADMINISTRATOR**) |
| `Firewall               opened 3880/tcp (console), 3843/tcp (agents), 3832/tcp (analytics uploads) (firewalld)` | firewalld is running (the ports are the ones in `forge.toml`) |
| `▲ Analytics worker       not listening on 3832 yet; see journalctl …` | The worker's upload port did not open within 30 s. The install still succeeds |
| `Analytics     off ([analytics] engine = "off")` | Analytics is off; there is no **Analytics worker** line |
| `✔ Agent                  enrolled in cluster <cluster>` and `This server   enrolled in cluster <cluster> as <hostname> (terminal account <account>)` | `--install-agent` succeeded |
| `▲ Agent                  not enrolled: <reason>; the server install is fine` | `--install-agent` failed. The **AGENT** section then shows **Retry by hand** commands |
| `✔ Elevated tokens        allowed: elevated API tokens skip the MFA prompt` | `allow_elevated_tokens = true` in `forge.toml` |
| `▲ Production ...` | The public URL in `forge.toml` does not start with `https://` |

:::caution
The summary prints the generated password in clear text. Run the installer
where nobody can read your screen or terminal log. The password stays in
`/var/lib/forge/initial-admin-password` until the administrator changes it
in **Profile → Password**.
:::

### Exit codes and errors

| Exit code | Message | Cause | Fix |
| --- | --- | --- | --- |
| `0` | | Installed or upgraded (also when only the `--install-agent` step failed), `--help`, or `n` on the review screen | |
| `2` | `unknown option <option> (see --help)` | An option the installer does not know, or `--option=value` | Use the forms in [Options](#options) |
| `1` | `--environment must be production, staging, development or test (got <value>).` | Invalid `--environment` | Pass one of the four values |
| `1` | `--mfa must be required or off (got <value>).` | Invalid `--mfa` | Pass `required` or `off` |
| `1` | `--allow-elevated-tokens must be yes or no (got <value>).` | Invalid `--allow-elevated-tokens` | Pass `yes` or `no` |
| `1` | `--terminal-account must use only a-z, 0-9, _ and - (got <value>).` | Invalid `--terminal-account` | Use a lower-case account name |
| `1` | `--cluster must use only A-Z, a-z, 0-9, ., _ and - (got <value>).` | Invalid `--cluster` | Use a name with those characters only |
| `1` | `Run as root: sudo ./install.sh …` | Not running as root | Run with `sudo` |
| `1` | `systemd is required; see the docs for running forge-server under another supervisor.` | No `/run/systemd/system` | Use a host with systemd, or run `forge-server --config <file> serve` under your own supervisor |
| `1` | `Run this from the extracted package (forge-server not found).` | No executable `forge-server` next to `install.sh` | Extract the full package and run its `install.sh` |
| `1` | `FORGE_ADMIN_PASSWORD must be at least 12 characters.` | Password too short | Use 12 or more characters, or unset it |
| `1` | `Could not generate a password.` | Reading `/dev/urandom` failed | Set `FORGE_ADMIN_PASSWORD` |
| `1` | `Could not set MFA in /etc/forge/forge.toml.` | `forge-server mfa` could not edit the file | Check the file with `forge-server --config /etc/forge/forge.toml mfa status` |
| `1` | `Could not create the administrator. Nothing was kept; fix the problem and run the installer again.` | `forge-server init` failed; its output is printed above, indented | Fix the reported problem and run again |
| `1` | `forge-server did not start. See: journalctl -u forge-server -n 50` | No answer from `/api/health` after 30 attempts | Read the journal; a common cause is another process on port 3880 or 3843 |
| non-zero | shell or command error | A command failed (`set -e`), or an option is missing its value | Read the message above it |

Errors go to standard error after a `✖` (or `x`) mark.

## get-forge.sh and get-forge.ps1

Download a release from GitHub (or take a package on disk), verify it, and
run its `install.sh` or `install.ps1`. Both are assets of every release.
Releases have no Windows package yet, so `get-forge.ps1` works only with
`-File` and a package you built yourself.

**Requires:** root (`get-forge.sh`) or PowerShell as Administrator
(`get-forge.ps1`); `curl`, `tar`, `gzip` and OpenSSL 1.1.1 or later on Linux
(`openssl` is installed with `apt-get`, `dnf`, `yum` or `zypper` when it is
missing); `openssl.exe` on Windows, from `PATH` or Git for Windows.

### Synopsis

```
sudo sh get-forge.sh [--version latest|<tag>] [--file <archive>] [--download-only] [--] [install.sh options]
sh get-forge.sh --verify-only <folder>
.\get-forge.ps1 [-Version latest|<tag>] [-File <zip>] [-DownloadOnly] [-SkipSignatureCheck] [install.ps1 parameters]
```

### Options

| Name | Default | Description | Example |
| --- | --- | --- | --- |
| `--version`, `-Version` | `latest` | `latest` is the newest **stable** release (`/releases/latest`); a pre-release needs its tag. `1.2.3` and `v1.2.3` both work | `--version v0.1.0-rc-03` |
| `--file`, `-File` | none | Install a package on disk. `SHA256SUMS` and `SHA256SUMS.sig` must be in the same folder | `--file ./forge-0.1.0-x86_64-linux.tar.gz` |
| `--download-only`, `-DownloadOnly` | off | Verify and unpack into the current folder; do not install | `--download-only` |
| `--verify-only <folder>` | | Linux: check `<folder>/SHA256SUMS` against its signature and every listed file present in the folder; install nothing | `--verify-only .` |
| `-SkipSignatureCheck` | off | Windows only: install after the checksum check alone, when no `openssl.exe` is available | `-SkipSignatureCheck` |
| other options, or anything after `--` | | Passed to `install.sh` / `install.ps1` unchanged | `-- --yes --analytics clickhouse` |

What they check, in order, before anything is installed:

1. `SHA256SUMS.sig` is a valid Ed25519 signature of `SHA256SUMS` by the
   Orbyn Forge release key, which is pinned in the script and equals
   `RELEASE_PUBLIC_KEY` in Forge.
2. The package's SHA-256 is listed in `SHA256SUMS` and matches.
3. The package is for this machine (`x86_64` or `aarch64` on Linux) and
   unpacks only into its own folder.

| Environment variable | Description |
| --- | --- |
| `FORGE_RELEASES_URL` | Release base URL, default `https://github.com/orbyntech/forge/releases`. A mirror must have the same `download/<tag>/<file>` and `latest/download/<file>` layout; `file://` and `http://` mirrors work, since the signature, not the transport, makes a download trusted |
| `FORGE_RELEASE_PUBLIC_KEY` | Trust another release key (base64url), for example a lab key. A warning is printed |

When `get-forge.sh` runs from a pipe and a terminal is attached, the
installer's questions read from `/dev/tty`.

## install.ps1 (Windows)

Installs or upgrades the control plane from the extracted Windows package.

**Requires:** PowerShell (Windows PowerShell 5.1 or later) run as
Administrator, and the extracted package as the working copy
(`forge-server.exe` next to `install.ps1`).

### Synopsis

```
.\install.ps1 [-PublicUrl <url>] [-AdminUsername <name>] [-AdminEmail <email>]
              [-AgentHost <name>] [-InstallDir <dir>] [-DataRoot <dir>]
              [-DbUrl <url>] [-DbName <name>] [-AllowSharedDb]
              [-Analytics off|clickhouse|duckdb] [-AnalyticsUrl <url>] [-AnalyticsName <name>]
.\install.ps1 -Uninstall [-Purge] [-InstallDir <dir>] [-DataRoot <dir>]
Get-Help .\install.ps1
```

### Parameters

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `-PublicUrl` | string (URL) | `http://<fqdn>:3880`, where `<fqdn>` is the DNS host name of `%COMPUTERNAME%` | No | URL users open. An `https://` URL also sets `cookie_secure = true`. First install only | `-PublicUrl https://<forge-host>` |
| `-AdminUsername` | string | `admin` | No | Sign-in name of the first platform administrator. First install only | `-AdminUsername <admin-name>` |
| `-AdminEmail` | string | none | No | Email of the first administrator. First install only | `-AdminEmail <admin-email>` |
| `-DbUrl` | string | none (SQLite) | No | PostgreSQL server without database name and password. Password in `FORGE_DATABASE_PASSWORD`, stored in `db-password` next to `forge.toml` (SYSTEM and Administrators only). First install only | `-DbUrl postgres://forge@db1:5432` |
| `-DbName` | string | `forge_op` | No | Database name; created when missing, reused when it exists | `-DbName forge_op` |
| `-AllowSharedDb` | switch | off | No | Uses a database that already holds other tables | `-AllowSharedDb` |
| `-Analytics` | `off`, `clickhouse` or `duckdb` | `off` | No | Analytics database. The embedded engine is Linux only; `duckdb` is accepted but not yet verified on Windows | `-Analytics clickhouse` |
| `-AnalyticsUrl` | string | none | With `-Analytics clickhouse` | ClickHouse address without password; password in `FORGE_ANALYTICS_PASSWORD` | `-AnalyticsUrl https://default@ch1:8443` |
| `-AnalyticsName` | string | `<DbName>_analytics` | No | Analytics database name | `-AnalyticsName forge_op_analytics` |
| `-AgentHost` | string | the host of `-PublicUrl` | No | Name or IP nodes use to reach this server. Sets `agent_public_url` and the first entry of `agent_hostnames`. First install only | `-AgentHost <forge-ip>` |
| `-InstallDir` | directory | `C:\Program Files\Orbyn Forge` | No | Binaries and agent binaries (`dist\`). Use the same value on every run | `-InstallDir "D:\Orbyn Forge"` |
| `-DataRoot` | directory | `C:\ProgramData\Orbyn Forge` | No | Holds `forge.toml` and `data\`. Use the same value on every run | `-DataRoot "D:\Forge Data"` |
| `-Uninstall` | switch | off | No | Stops and removes the **Orbyn Forge** task, the processes, `-InstallDir` and the **Orbyn Forge &lt;port&gt;** firewall rules; keeps `-DataRoot` | `-Uninstall` |
| `-Purge` | switch | off | With `-Uninstall` | Also deletes `-DataRoot` (configuration, database, keys, CA) | `-Uninstall -Purge` |

Before it changes anything, `install.ps1` checks that the package has
`forge-server.exe`, `forge-plugin-runner.exe`, `forge-analytics-worker.exe`,
`dist\SHA256SUMS` and `dist\MANIFEST.json`, and that every agent binary
matches `dist\SHA256SUMS`. It opens the ports of `api_listen`,
`agent_listen` and `[analytics] worker_agent_listen` from `forge.toml`
(default `3880`, `3843` and `3832`).

`install.ps1` has no environment or MFA parameter. The `forge.toml` it writes
has no `[security]` section, so MFA is each user's choice (the default). To
change either, edit `forge.toml` or run `forge-server mfa on`, then restart the
task. See [Installation](/docs/getting-started/installation/).

### Environment variables

| Name | Default | Description |
| --- | --- | --- |
| `FORGE_ADMIN_PASSWORD` | unset: the installer generates a password | Password of the first administrator, at least 12 characters. When it is set, nothing is saved to disk. The installer removes the variable from the session after it creates the administrator |

### Examples

Install with the defaults:

```powershell
.\install.ps1
```

Install behind a TLS proxy, with a named administrator:

```powershell
.\install.ps1 -PublicUrl https://<forge-host> -AdminUsername <admin-name> -AdminEmail <admin-email>
```

Choose the administrator password yourself, without putting it on a command
line:

```powershell
$env:FORGE_ADMIN_PASSWORD = [System.Net.NetworkCredential]::new("", (Read-Host -AsSecureString "Administrator password")).Password
.\install.ps1 -PublicUrl https://<forge-host>
```

Install to another drive:

```powershell
.\install.ps1 -InstallDir "D:\Orbyn Forge" -DataRoot "D:\Forge Data"
```

### What it does

| Line | What happens |
| --- | --- |
| (no line) | Stops the scheduled task **Orbyn Forge** if it exists, ends every `forge-server` process, and waits one second, so the files can be replaced |
| **Directories** | Creates the install directory, `<InstallDir>\dist` and `<DataRoot>\data`. Removes inherited permissions from `<DataRoot>` and grants full control to `SYSTEM` and `Administrators` only |
| **Binaries** | Copies `forge-server.exe`, `forge-plugin-runner.exe`, `forge-analytics-worker.exe` and every file in the package's `dist\`. The line counts the `forge-agent*` files |
| **Configuration** | Writes `<DataRoot>\forge.toml` if it does not exist. An existing file is kept |
| **Administrator** | Creates the first platform administrator when `<DataRoot>\data\forge.db` does not exist and `forge.toml` has no `database_url` line. Same rules as on Linux; the password file is `<DataRoot>\data\initial-admin-password` |
| **Startup task** | Registers (or replaces) the scheduled task **Orbyn Forge** and starts it |
| **Firewall** | Creates the inbound Windows Firewall rules **Orbyn Forge &lt;port&gt;** (TCP, allow) for the ports of `api_listen`, `agent_listen` and `[analytics] worker_agent_listen` in `forge.toml` (default **Orbyn Forge 3880**, **3843** and **3832**) unless rules with those names exist |
| **Health check** | Requests `http://127.0.0.1:3880/api/health` (2-second timeout), up to 30 attempts one second apart |

### Files and services it creates

| Path or object | Access | Contents |
| --- | --- | --- |
| `C:\Program Files\Orbyn Forge\forge-server.exe`, `forge-plugin-runner.exe`, `forge-analytics-worker.exe` | inherited | Control plane, plugin runner and analytics worker |
| `C:\Program Files\Orbyn Forge\dist\` | inherited | Agent binaries handed to nodes, and `SHA256SUMS` |
| `C:\ProgramData\Orbyn Forge\forge.toml` | SYSTEM, Administrators | Configuration (first install only) |
| `C:\ProgramData\Orbyn Forge\data\` | SYSTEM, Administrators | Database, keys, internal CA, backups |
| `C:\ProgramData\Orbyn Forge\data\initial-admin-password` | SYSTEM, Administrators | Generated administrator login. Forge deletes it when the administrator changes the password |
| Scheduled task **Orbyn Forge** | | Runs `forge-server.exe --config "<DataRoot>\forge.toml" serve` in `<InstallDir>` as `SYSTEM` with highest privileges, at startup. No time limit; restarts every minute after a failure, up to 999 times; starts when available; runs on battery |
| Firewall rules **Orbyn Forge 3880**, **Orbyn Forge 3843** | | Inbound TCP allow, all profiles |

The `forge.toml` it writes on a first install:

```toml
# Orbyn Forge control plane (written by install.ps1; see forge.toml.example).
data_dir = "C:/ProgramData/Orbyn Forge/data"
api_listen = "0.0.0.0:3880"
public_url = "http://<fqdn>:3880"
cookie_secure = false
agent_listen = "0.0.0.0:3843"
agent_public_url = "https://<fqdn>:3843"
agent_hostnames = ["<fqdn>", "<fqdn>", "localhost", "127.0.0.1"]
agent_dist_dir = "C:/Program Files/Orbyn Forge/dist"

[update]
enabled = true
```

Fresh install and upgrade differ as on Linux: an upgrade keeps `forge.toml`
and the accounts, re-registers the task, and prints `upgraded to <version>
and running` without the **ADMINISTRATOR** section.

### What install.ps1 prints

A new installation:

```
  ╭─ Orbyn Forge · control plane installer
  │  New installation  ·  version <version>  ·  host <fqdn>
  ╰────────────────────────────────────────────────────────────

  ✔ Directories            C:\Program Files\Orbyn Forge · C:\ProgramData\Orbyn Forge
  ✔ Binaries               server, analytics worker, plugin runner, 5 agent binaries
  ✔ Configuration          C:\ProgramData\Orbyn Forge\forge.toml (new)
  ✔ Administrator          admin (platform admin)
  ✔ Startup task           'Orbyn Forge' (SYSTEM, at boot, restarts on failure)
  ✔ Firewall               inbound TCP 3880 and 3843 allowed
  ✔ Health check           responding after 1s

  Orbyn Forge <version> is installed and running.

  ACCESS
    Console       http://<fqdn>:3880
    Agents        https://<fqdn>:3843  (nodes connect out to TCP 3843 and 3832)
    Analytics     duckdb, uploads on port 3832

  ADMINISTRATOR
    Username      admin
    Password      <generated-password>
    Saved to      C:\ProgramData\Orbyn Forge\data\initial-admin-password  (Administrators only, deleted when you change it)
    · Sign in, then in Profile add your email, change the password and set up MFA.

  INSTALLED
    Binaries      C:\Program Files\Orbyn Forge
    Agents        C:\Program Files\Orbyn Forge\dist
    Config        C:\ProgramData\Orbyn Forge\forge.toml
    Data          C:\ProgramData\Orbyn Forge\data  (database, keys, internal CA, backups)
    Startup       scheduled task 'Orbyn Forge'  (SYSTEM)

  OPERATE
    Status        Get-ScheduledTask -TaskName 'Orbyn Forge' | Get-ScheduledTaskInfo
    Restart       Stop-ScheduledTask 'Orbyn Forge'; Start-ScheduledTask 'Orbyn Forge'

  ▲ Production             Put a TLS proxy (Caddy, Nginx, IIS) in front and use an https:// public URL.

```

The **Health check** count is the number of failed attempts before the
first answer, so `0s` is possible.

### Exit codes and errors

| Exit code | Message | Cause | Fix |
| --- | --- | --- | --- |
| `0` | | Installed or upgraded | |
| `1` | `Run PowerShell as Administrator.` | Not elevated | Open PowerShell with **Run as administrator** |
| `1` | `Run this from the extracted package (forge-server.exe not found).` | No `forge-server.exe` next to `install.ps1` | Extract the full zip and run its `install.ps1` |
| `1` | `FORGE_ADMIN_PASSWORD must be at least 12 characters.` | Password too short | Use 12 or more characters, or remove the variable |
| `1` | `Could not create the administrator. Nothing was kept; fix the problem and run the installer again.` | `forge-server init` failed; its output is printed above | Fix the reported problem and run again |
| `1` | `forge-server did not start. Run it by hand to see why: & '<InstallDir>\forge-server.exe' --config '<DataRoot>\forge.toml' serve` | No answer from `/api/health` after 30 attempts | Run the printed command to see the error |
| non-zero | PowerShell error | A cmdlet failed (`$ErrorActionPreference = 'Stop'`), for example an invalid `-PublicUrl` | Read the error record |

If PowerShell refuses to run the script because it came from the internet,
unblock the package files first:

```powershell
Get-ChildItem -Recurse | Unblock-File
```

## See also

- [Installation](/docs/getting-started/installation/): download, install,
  update and uninstall, step by step.
- [Configuration](/docs/reference/configuration/): every `forge.toml` key.
- [Node installer](/docs/reference/cli/node-installer/): the script nodes run to
  install the agent.
- [Command line](/docs/reference/cli/): `forge-server init`, `mfa` and the other
  subcommands.
- [MFA is your choice](/docs/security/#mfa-is-your-choice) and
  [Hardening](/docs/security/hardening/).
