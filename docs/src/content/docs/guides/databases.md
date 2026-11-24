---
title: Choose and connect databases
description: Pick SQLite or PostgreSQL for the operational database and an engine for analytics, at install time or later.
sidebar:
  order: 60
---

This guide shows you how to choose where Forge keeps its data. Forge has two
databases:

- The **operational** database holds users, access, nodes, apps, jobs and the
  audit log. It is SQLite (default) or PostgreSQL.
- The **analytics** database holds node history. It is the embedded engine
  (ClickHouse run by Forge on the same machine), a ClickHouse server, or off.

For the background, see [Analytics](/docs/concepts/analytics/). For every key, see
[Configuration](/docs/reference/configuration/#databases).

## Before you begin

- You install Forge as `root` (Linux) or as Administrator (Windows).
- For a PostgreSQL server you run yourself: PostgreSQL 13 or later, and a
  login role. To let Forge create the database, the role needs `CREATEDB`.
  Otherwise an administrator creates the database first.
- The embedded engine runs on Linux `x86_64` and `aarch64`. On Windows,
  choose `clickhouse` or `off` for analytics.

| Setup | Operational | Analytics | Use it for |
| --- | --- | --- | --- |
| Single box | SQLite | Embedded | Labs and small fleets; nothing else to install |
| Typical | PostgreSQL | Embedded | Teams that already run PostgreSQL |
| Large | PostgreSQL | ClickHouse | Large fleets and high log volume |

## Steps

### Use PostgreSQL on this machine

1. Run the installer with `--db-local`. It installs PostgreSQL from the
   system packages, creates a login role `forge` with a generated password
   and stores that password in `/etc/forge/db-password` (mode `0600`).

   ```sh
   sudo ./install.sh --db-local --db-name forge_op
   ```

### Use an existing PostgreSQL server

1. Put the password in the environment. Never put it in the URL.
2. Run the installer with the server address and the database name.

   ```sh
   export FORGE_DATABASE_PASSWORD='<password>'
   sudo -E ./install.sh --db-url postgres://forge@<db-host>:5432 --db-name forge_op
   ```

   On a Windows control plane you built from source:

   ```powershell
   $env:FORGE_DATABASE_PASSWORD = '<password>'
   .\install.ps1 -DbUrl postgres://forge@<db-host>:5432 -DbName forge_op
   ```

The installer runs `forge-server db check --create`:

- A missing database is created.
- An existing empty database, or one that already holds Forge, is reused.
  Forge never drops or recreates a database.
- A database with other tables is refused unless you pass
  `--allow-shared-db` (`-AllowSharedDb` on Windows).

### Choose the analytics database

1. Pass `--analytics` with `duckdb` (the default), `embedded`, `clickhouse` or `off`. For
   `clickhouse`, add `--analytics-url` and put the password in
   `FORGE_ANALYTICS_PASSWORD`. `duckdb` needs no download and no server:
   it suits installs up to about 100 nodes with moderate logging (see
   [The DuckDB store](/docs/concepts/analytics/#the-duckdb-store) and the
   [benchmark](/docs/development/benchmarks/#analytics-duckdb-against-chdb)).

   ```sh
   export FORGE_ANALYTICS_PASSWORD='<password>'
   sudo -E ./install.sh --db-local --analytics clickhouse \
     --analytics-url https://default@<clickhouse-host>:8443
   ```

The analytics database is named after the operational one with
`_analytics` added (`forge_op_analytics`), unless you pass
`--analytics-name`.

### Install the embedded engine

The embedded engine is a separate download, so `forge-server` stays small.

1. Download the ClickHouse engine binary for your architecture and note its
   SHA-256 from the same source.
2. Install it. Forge checks the hash before it copies the file.

   ```sh
   sudo -u forge /opt/forge/forge-server --config /etc/forge/forge.toml \
     analytics install --binary /tmp/clickhouse --sha256 <sha256>
   ```

3. Restart Forge. It starts the engine on `127.0.0.1:18123` with a generated
   password and a 1 GiB memory budget (`[analytics] embedded_memory_mb`). You
   can change the budget and a CPU cap later without
   editing `forge.toml`; see
   [Configure analytics retention and limits](/docs/guides/analytics-settings/).

   ```sh
   sudo systemctl restart forge-server
   ```

The interactive installer asks the same questions when you run it on a
terminal without options.

## Verify

1. Check the analytics database. `ready` is `true` once the schema is in
   place, about a minute after start.

   ```sh
   forge nodes analytics
   ```


2. Show the settings. Passwords appear as `***`.

   ```sh
   sudo -u forge /opt/forge/forge-server --config /etc/forge/forge.toml db show
   ```

3. Check the operational database. It prints `usable: yes` and exits `0`.

   ```sh
   sudo -u forge /opt/forge/forge-server --config /etc/forge/forge.toml db check
   ```

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `missing, and this role may not create databases` | The role lacks `CREATEDB` | Ask an administrator to run `CREATE DATABASE forge_op OWNER <role>;`, then re-run |
| `tables that are not Forge's` | The database holds another service | Use another `--db-name`, or pass `--allow-shared-db` |
| `Forge tables from a newer Forge build` | The database was used by a newer Forge | Install that Forge version or newer |
| `put the database password in password_file or the environment` | The URL contains a password | Remove it from the URL and use `FORGE_DATABASE_PASSWORD` |
| `cannot reach …` | Network, firewall or `pg_hba.conf` | Allow the Forge server's address in `pg_hba.conf` and the firewall |
| `the embedded analytics engine is not installed` in the log | No engine binary | Run `forge-server analytics install` |
| `forge nodes analytics` shows `ready: false` | The engine is starting, or the ClickHouse server cannot be reached | Read `last_error`; check the URL, password and firewall |

## Next steps

- [Configuration reference](/docs/reference/configuration/#databases)
- [`forge-server db`](/docs/reference/cli/forge-server/#forge-server-db)
- [Analytics concepts](/docs/concepts/analytics/)
