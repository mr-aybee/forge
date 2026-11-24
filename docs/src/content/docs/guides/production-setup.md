---
title: Run Forge in production
description: Put TLS in front of Forge, require MFA, choose PostgreSQL and back up the data directory of a production control plane.
sidebar:
  order: 5
---

This guide takes a control plane installed with the defaults and makes it
ready for production: public TLS, required MFA, the right databases and
backups. For the trade-offs behind these settings, see the
[Security model](/docs/concepts/security-model/).

## Before you begin

- Forge is installed. See [Installation](/docs/getting-started/installation/).
- You have root on the server (Administrator on Windows) and a DNS name for
  it, for example `forge.example.com`.
- You have a TLS reverse proxy you can run on the server, such as Caddy or
  Nginx (IIS on Windows).

## 1. Put a TLS proxy in front of the console

```diagram
alt: Forge behind Caddy
flow:
  direction: column
  steps:
    - { label: Internet, tone: muted, edge: "443" }
    - { label: Caddy, note: "public TLS, HTTP/2", edge: "`127.0.0.1:3880`" }
    - { label: forge-server, note: "+ SQLite · agents dial in on `:3843` and `:3832` (mTLS)", tone: accent }
```

1. Set these keys in `/etc/forge/forge.toml`
   (`C:\ProgramData\Orbyn Forge\forge.toml` on Windows):

   ```toml
   api_listen = "127.0.0.1:3880"
   public_url = "https://forge.example.com"
   cookie_secure = true
   ```

2. Point the proxy at `127.0.0.1:3880`. With Caddy:

   ```text
   forge.example.com {
     reverse_proxy 127.0.0.1:3880
   }
   ```

3. Restart Forge: `sudo systemctl restart forge-server`.

`agent_listen` (`3843`) and the analytics upload port (`3832`) stay as they
are: they use Forge's internal CA and need no public certificate. Add every
name nodes use to reach the server to `agent_hostnames`. To publish Forge
through a cluster ingress edge instead, see
[Publish Forge on a domain](/docs/guides/publish-forge-on-a-domain/).

## 2. Require MFA for guarded actions

By default MFA is each user's choice: a user who enrolls a factor is asked
for it at sign-in, and no action asks for MFA. Requiring it also makes
sensitive actions, such as opening a terminal or running a command on a
node, ask for an MFA-verified session.

```sh
sudo /opt/forge/forge-server --config /etc/forge/forge.toml mfa on
sudo systemctl restart forge-server
```

On Windows:

```powershell
& "C:\Program Files\Orbyn Forge\forge-server.exe" --config "C:\ProgramData\Orbyn Forge\forge.toml" mfa on
Stop-ScheduledTask -TaskName "Orbyn Forge"; Start-ScheduledTask -TaskName "Orbyn Forge"
```

`mfa status` shows the setting, and `mfa off` turns it off. The command edits
`forge.toml` in place and keeps its comments, owner and mode. On Linux you
can also choose it at install time with `--mfa required`. See
[MFA is your choice](/docs/security/#mfa-is-your-choice).

:::caution
With MFA off, one stolen password gives root on every node that user can
reach, unless that user enrolled MFA for sign-in. Require MFA on any Forge
that clients or production systems depend on.
:::

## 3. Choose the databases

- **Operational database:** SQLite is built in and suits most installs. For
  large installs, choose PostgreSQL when you install: `--db-local` installs
  PostgreSQL on the same machine, and `--db-url postgres://forge@<db-host>:5432`
  with the password in `FORGE_DATABASE_PASSWORD` uses an existing server.
- **Analytics database:** see
  [Choose an analytics engine](/docs/getting-started/installation/#choose-an-analytics-engine).

Both are covered in [Choose and connect databases](/docs/guides/databases/).

## 4. Back up the data directory

Everything stateful lives in `data_dir` (`/var/lib/forge`, or
`C:\ProgramData\Orbyn Forge\data`):

| Path | Contents | Sensitivity |
| --- | --- | --- |
| `forge.db` (+ `-wal`, `-shm`) | SQLite database | High |
| `keys/master.key` | Key-encryption key for secrets | **Critical** |
| `keys/audit.key` | Audit signing key | **Critical** |
| `keys/command.key` | Agent command signing key | **Critical** |
| `pki/ca.key`, `pki/ca.crt` | Internal CA | **Critical** (key) |
| `plugins/` | Verified plugin artifacts | Medium |
| `backups/` | Database copies taken before each update | High |

1. Stop the server, or use SQLite's online backup for `forge.db`.
2. Copy `data_dir` as a unit.
3. Store copies of `keys/` and `pki/` separately and encrypted.

:::caution
Restoring the database without its keys makes secrets and audit signatures
unverifiable.
:::

The Linux installer creates `/var/lib/forge` with mode `0700`, owned by
`forge`. On Windows the folder allows only SYSTEM and Administrators.

## Verify

- `https://forge.example.com` opens the console with a valid certificate.
- `sudo /opt/forge/forge-server --config /etc/forge/forge.toml mfa status`
  reports MFA as required.
- A restore of your backup on a test machine starts and shows your nodes.

## Next steps

- [Hardening](/docs/security/hardening/)
- [Update Forge](/docs/guides/update-forge/)
- [Network ports](/docs/reference/network-ports/)
