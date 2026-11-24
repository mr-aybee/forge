---
title: forge-server
description: Every forge-server command and flag, with types, defaults, examples, sample output and exit codes.
sidebar:
  order: 90
---

`forge-server` is the Orbyn Forge control plane. One binary runs the server
(`serve`), creates the first administrator (`init`), recovers access
(`reset-mfa`), switches MFA on or off (`mfa`), sets the elevated-token policy
(`elevated-tokens`), checks the database schema (`check-migrations`), makes an
enrollment token for a node on this host (`enroll-token`), prints the
configuration (`config`) and signs releases in CI (`release`).

On a Linux package install the binary is `/opt/forge/forge-server`, the
configuration is `/etc/forge/forge.toml` and the service runs as the `forge`
user. On Windows it is `C:\Program Files\Orbyn Forge\forge-server.exe` with
`C:\ProgramData\Orbyn Forge\forge.toml`, and the server runs as SYSTEM (a
scheduled task). Run maintenance commands as the service user (Linux) or from
an elevated prompt (Windows), so files in the data directory keep their owner.

## Synopsis

```text
Usage: forge-server [OPTIONS] [COMMAND]

Commands:
  serve      Runs the control plane (default)
  init       Creates the first platform administrator. The password is read from FORGE_ADMIN_PASSWORD or, if unset, from the first line of stdin
  reset-mfa  Recovery on the server host: removes a user's passkeys and authenticator app, revokes their API keys and signs out all their sessions, so they can sign in with their password and enroll again. Audited
  config     Prints the effective configuration
  mfa               Whether MFA is required on this server: `status`, `on` or `off`. Edits `[security] require_mfa` in forge.toml; restart to apply
  elevated-tokens   Whether API keys and service-account tokens marked elevated may run MFA-guarded actions: `status`, `on` or `off`. Edits `[security] allow_elevated_tokens` in forge.toml; restart to apply
  public-address    The public address people use in links (alert emails, webhooks, approvals): `get`, `set <url>` or `clear`. Stored in the database, like the console's Settings page; `clear` falls back to `public_url`. Audited as system:forge. Restart a running server to apply it there
  check-migrations  Exits non-zero when the database has applied migrations this binary does not know (it is older than the database). Used by the self-update dry run; never changes the database
  analytics         The embedded analytics engine: install its binary
  db                The operational database: test it, create it, show the configuration
  enroll-token      Creates an enrollment token for a node on this host (installer use). Creates the cluster if it is missing. Prints only the token on stdout. Run as root on the server host; audited as system:forge
  release    Release signing (CI): `keygen` prints a new key pair; `sign <file>` prints the base64 signature of a file (key in FORGE_RELEASE_KEY)
  help       Print this message or the help of the given subcommand(s)

Options:
  -c, --config <CONFIG>  Path to forge.toml [env: FORGE_CONFIG=]
      --json-logs        Log as JSON lines
  -h, --help             Print help
  -V, --version          Print version
```

## Global options

Every command accepts these options, before or after the command name.

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `-c`, `--config <CONFIG>` | path | none: built-in defaults plus `FORGE_*` variables | No (`mfa` needs it) | Path to `forge.toml`. A path that does not exist stops the command with `config file <path> not found`. See [Configuration](/docs/reference/configuration/) for every key and its `FORGE_*` variable | `--config /etc/forge/forge.toml` |
| `--json-logs` | flag | off | No | Write log lines as JSON instead of text | `--json-logs` |
| `-h`, `--help` | flag | | No | Print the help of the command | `forge-server mfa --help` |
| `-V`, `--version` | flag | | No | Print the version | `forge-server --version` |

| Environment variable | Used by | Description |
| --- | --- | --- |
| `FORGE_CONFIG` | all | Same as `--config` |
| `RUST_LOG` | all | Log filter, such as `info` or `forge_server=debug`. Default `info,sqlx=warn` |
| `FORGE_*` keys | all | Override single configuration keys; see [Configuration](/docs/reference/configuration/) |
| `FORGE_ADMIN_PASSWORD` | `init` | The administrator's password; without it, `init` reads the first line of stdin |
| `FORGE_RELEASE_KEY` | `release sign`, `release pubkey` | The release-signing private key |

Log lines go to standard output, mixed with a command's own output. Filter
them with `RUST_LOG=error` when you script a command.

**Exit codes**

| Code | Meaning |
| --- | --- |
| `0` | Success |
| `1` | The command failed. The reason follows `Error:` on standard error |
| `2` | A usage error: an unknown command or flag, a missing or invalid argument |

## forge-server serve

Runs the control plane: the API and console listener, the agent listener and
the background work (jobs, reconcilers, updates). It is the default when no
command is given. Runs as the service user under systemd or as a Windows
service.

```text
Usage: forge-server serve [OPTIONS]
```

No options besides the [global options](#global-options).

**Examples**

```sh
# What the systemd unit runs
/opt/forge/forge-server --config /etc/forge/forge.toml serve

# Run in the foreground for a quick test, with debug logs
RUST_LOG=forge_server=debug ./forge-server --config ./forge.toml
```

**Sample output** (log lines at start):

```text
INFO forge_server: Orbyn Forge control plane started api=0.0.0.0:3880 agents=0.0.0.0:3843 version="<version>"
```

An info line follows at every start:
`MFA policy required_for_guarded_actions=<true|false>`. After
the MFA setting changes, the next start logs `MFA setting old=… new=…` and
records `config.mfa` in the audit log. It stops on `SIGTERM` or Ctrl+C
(`shutting down restart=false`) and restarts itself after a console update.

## forge-server init

Creates the first platform administrator and the default workspace. It
refuses when any user exists. The installer runs it once; run it yourself only
for a manual install.

```text
Usage: forge-server init [OPTIONS]

Options:
  -c, --config <CONFIG>      Path to forge.toml [env: FORGE_CONFIG=]
      --username <USERNAME>  Sign-in name (default `admin` when no email is given either)
      --email <EMAIL>        Optional email; it can also be added later in the console
      --json-logs            Log as JSON lines
      --name <NAME>          [default: Administrator]
      --generated-password   The installer generated the password and saved it to `<data_dir>/initial-admin-password`; Forge deletes that file when the administrator changes their password
  -h, --help                 Print help
```

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--username <USERNAME>` | string | `admin` when `--email` is not given either | No | Sign-in name: letters, digits, `.`, `_` or `-`, 3 to 64 characters | `--username ops-admin` |
| `--email <EMAIL>` | string | none | No | Email to sign in with as well | `--email <admin-email>` |
| `--name <NAME>` | string | `Administrator` | No | Display name | `--name "Platform admin"` |
| `--generated-password` | flag | off | No | Marks the password as the one saved in `<data_dir>/initial-admin-password`; Forge deletes that file when the administrator changes the password | `--generated-password` |
| `FORGE_ADMIN_PASSWORD` | env | stdin | No | The password, at least 12 characters. Without it, `init` reads the first line of stdin | |

The sign-in name and password are checked before the database is created, so
a rejected password leaves nothing behind.

**Examples**

```sh
# Password from the environment (Linux package install)
sudo -u forge env FORGE_ADMIN_PASSWORD='<password>' \
  /opt/forge/forge-server --config /etc/forge/forge.toml init --username admin

# Password from stdin, with an email as a second sign-in name
printf '%s\n' '<password>' | sudo -u forge \
  /opt/forge/forge-server --config /etc/forge/forge.toml init --username admin --email <admin-email>
```

**Sample output**

```text
created platform admin admin (<user-id>) and default org <workspace-id>
```

A second run fails with exit code `1`:

```text
Error: already initialised: users exist
```

## forge-server reset-mfa

Recovery on the server host when a person has lost every MFA method. It
removes all of the user's passkeys and their authenticator app (TOTP),
revokes their API keys and signs out every session of that user, so they can
sign in with their password and enroll again. Audited as `auth.mfa.reset` (actor `system:forge`).

```text
Usage: forge-server reset-mfa [OPTIONS] <LOGIN>

Arguments:
  <LOGIN>  Username or email
```

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<LOGIN>` | string | | Yes | Username, or email when it contains `@`. Case-insensitive | `alice` |

**Examples**

```sh
sudo -u forge /opt/forge/forge-server --config /etc/forge/forge.toml reset-mfa <username-or-email>
```

**Sample output**

```text
alice: removed 1 passkey(s) and the authenticator app, revoked 0 API key(s); all sessions signed out. Sign in with the password and enroll MFA again.
```

No match fails with exit code `1`: ``Error: no user `<login>` ``. See
[Passkeys](/docs/guides/passkeys/#recover-access).

## forge-server config

Prints the effective configuration: `forge.toml` with every `FORGE_*`
override applied and every default filled in. Use it to check what the server
will run with. It prints no secrets: the configuration holds none (the OIDC
secret is read from the environment variable named in `oidc.client_secret_env`).

```text
Usage: forge-server config [OPTIONS]
```

**Examples**

```sh
sudo -u forge /opt/forge/forge-server --config /etc/forge/forge.toml config
```

**Sample output** (shortened):

```text
ServerConfig {
    data_dir: "/var/lib/forge",
    database_url: None,
    api_listen: 0.0.0.0:3880,
    agent_listen: 0.0.0.0:3843,
    public_url: "https://<forge-host>",
    agent_public_url: "https://<forge-host>:3843",
    cookie_secure: true,
    environment: Production,
    security: SecuritySettings {
        require_mfa: false,
        login_challenge: true,
        captcha_after_failures: 2,
        refresh_token_days: 30,
        trusted_proxies: [],
    },
    ...
}
```

## forge-server mfa

Shows or changes whether MFA-guarded actions need an MFA-verified session
(`[security] require_mfa`, off by default). Users who enrolled MFA are asked
for it at sign-in either way. `on` and `off` edit `forge.toml` in place:
comments, owner and mode are kept, the file is replaced atomically, and a
deprecated `dev_skip_mfa` line is set to `false`. Restart the server to apply
a change. Run it as root or the service user, with write access to the file.

```text
Usage: forge-server mfa [OPTIONS] [ACTION]

Arguments:
  [ACTION]  [default: status] [possible values: status, on, off]
```

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `[ACTION]` | `status`, `on` or `off` | `status` | No | `status` prints the current state; `on` requires MFA; `off` stops requiring it | `off` |
| `--config` | path | | **Yes** | The `forge.toml` to read or edit (or `FORGE_CONFIG`) | `--config /etc/forge/forge.toml` |

**Examples**

```sh
sudo /opt/forge/forge-server --config /etc/forge/forge.toml mfa
sudo /opt/forge/forge-server --config /etc/forge/forge.toml mfa on
sudo systemctl restart forge-server
```

**Sample output**

```text
$ forge-server --config /etc/forge/forge.toml mfa status
MFA is not required (/etc/forge/forge.toml).

$ forge-server --config /etc/forge/forge.toml mfa on
MFA is now required in /etc/forge/forge.toml.
Restart forge-server to apply it: systemctl restart forge-server
```

When `FORGE_REQUIRE_MFA` or `FORGE_DEV_SKIP_MFA` disagrees with the file,
`status` adds `The file says …; an environment variable overrides it.` and
`on`/`off` add `Note: FORGE_REQUIRE_MFA or FORGE_DEV_SKIP_MFA is set and
overrides the file.` Running it again with the same value prints `MFA is
already … in <path>.`

Without `--config` it fails with exit code `1`:

```text
Error: pass --config /etc/forge/forge.toml (or set FORGE_CONFIG): the setting lives there
```

An action other than `status`, `on` or `off` is a usage error (exit code
`2`): ``error: invalid value 'maybe' for '[ACTION]'``. See
[MFA is your choice](/docs/security/#mfa-is-your-choice).

## forge-server elevated-tokens

Shows or changes whether API keys and service-account tokens marked elevated
may run MFA-guarded actions (`[security] allow_elevated_tokens`). `on` and
`off` edit `forge.toml` in place, like [`mfa`](#forge-server-mfa). Restart the
server to apply a change. With `off`, nobody can create an elevated token and
existing ones act as normal tokens.

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `[ACTION]` | `status`, `on` or `off` | `status` | No | `status` prints the current state; `on` allows elevated tokens; `off` turns them off | `off` |
| `--config` | path | | **Yes** | The `forge.toml` to read or edit (or `FORGE_CONFIG`) | `--config /etc/forge/forge.toml` |

**Examples**

```sh
sudo /opt/forge/forge-server --config /etc/forge/forge.toml elevated-tokens off
sudo systemctl restart forge-server
```

**Sample output**

```text
$ forge-server --config /etc/forge/forge.toml elevated-tokens off
Elevated tokens are now not allowed in /etc/forge/forge.toml.
Restart forge-server to apply it: systemctl restart forge-server
```

## forge-server public-address

Shows, sets or clears the [public address](/docs/reference/configuration/#public-address):
the URL Forge puts in links people follow. It writes the database directly,
so it works without a signed-in session. Run it on the server host as the
service user. A running server reads the setting at start: restart it after
a change. The console and `forge settings public-address` apply at once.
Forge audits a change as `server.public_address.update` or
`server.public_address.clear`, actor `system:forge`.

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `get` | subcommand | | One of the three | Prints the address links use and where it comes from | `get` |
| `set <URL>` | subcommand, string | | One of the three | Sets it. An `http` or `https` origin: host and optional port, no path, query or user name | `set https://forge.example.com` |
| `clear` | subcommand | | One of the three | Removes the setting; links use `public_url` from `forge.toml` | `clear` |
| `--config` | path | built-in defaults | No | The `forge.toml` whose database to change (or `FORGE_CONFIG`) | `--config /etc/forge/forge.toml` |

**Examples**

```sh
sudo runuser -u forge -- /opt/forge/forge-server --config /etc/forge/forge.toml public-address set https://forge.example.com
sudo systemctl restart forge-server
```

**Sample output**

```text
$ forge-server --config /etc/forge/forge.toml public-address get
http://192.0.2.10:3880 (public_url in forge.toml; no setting)

$ forge-server --config /etc/forge/forge.toml public-address set https://forge.example.com
Links now use https://forge.example.com. Restart forge-server to apply it to a running server: systemctl restart forge-server

$ forge-server --config /etc/forge/forge.toml public-address set https://forge.example.com/forge
Error: https://forge.example.com/forge: use the origin only (scheme, host and port): no path, query or fragment
```

The last command exits with `1` and changes nothing.

### forge-server public-address get

Prints the address links use and where it comes from: the setting, or
`public_url` from `forge.toml`. Takes no arguments.

### forge-server public-address set

Sets the public address to `<URL>` after checking it. Exits with `1` when the
URL is refused.

### forge-server public-address clear

Removes the setting; links use `public_url`. Takes no arguments.

## forge-server check-migrations

Checks that this binary knows every migration the database has applied. It
exits with `3` when the database is newer than the binary, for example after
someone restored an old binary over a newer database. It never changes the
database. The self-update dry run calls it on the new binary before it swaps
any file.

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--config` | path | built-in defaults | No | The `forge.toml` whose database to read | `--config /etc/forge/forge.toml` |

**Sample output**

```text
$ forge-server --config /etc/forge/forge.toml check-migrations
every applied migration is known to forge-server <version>

$ forge-server --config /etc/forge/forge.toml check-migrations; echo $?
the database has migrations this forge-server <version> does not know: 20261201000000; it is older than the database
3
```

## forge-server analytics

Manages the embedded analytics engine. See
[Choose and connect databases](/docs/guides/databases/).

### forge-server analytics install

Copies the engine binary into `<data_dir>/analytics/bin/clickhouse` after
it checks the SHA-256. It refuses a file whose hash differs. Restart
`forge-server` afterwards; it then starts the engine on `127.0.0.1`.

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--binary` | path | none | Yes | The downloaded engine binary | `--binary /tmp/clickhouse` |
| `--sha256` | hex | none | Yes | Its expected SHA-256 | `--sha256 3f1c…` |

**Sample output**

```text
$ sudo -u forge /opt/forge/forge-server --config /etc/forge/forge.toml analytics install --binary /tmp/clickhouse --sha256 <sha256>
installed the analytics engine at /var/lib/forge/analytics/bin/clickhouse
set [analytics] engine = "embedded" and restart forge-server: systemctl restart forge-server
```

## forge-server db

Tests, creates and shows the databases Forge uses. The commands never print
passwords. See [Databases](/docs/guides/databases/) for the full setup.

### forge-server db check

Connects to the operational database and reports what it holds. It changes
nothing unless you pass `--create`. It exits `0` when Forge can use the
database and `4` when it cannot.

| Contents | Meaning | Usable |
| --- | --- | --- |
| `missing` | The database does not exist yet | Yes, when the role may create databases |
| `empty` | It exists and has no tables | Yes |
| `forge` | Forge tables this build knows; migrations update them | Yes |
| `forge_newer` | Forge tables from a newer Forge build | No: upgrade this server first |
| `foreign` | Tables that are not Forge's | Only with `--allow-shared` |

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--create` | flag | off | No | Create the PostgreSQL database when it is missing. An existing database is never recreated | `--create` |
| `--allow-shared` | flag | off | No | Accept a database that also holds tables that are not Forge's | `--allow-shared` |
| `--json` | flag | off | No | Print the report as one JSON object | `--json` |

**Sample output**

```text
$ forge-server --config /etc/forge/forge.toml db check --create
database: postgres://forge:***@db1:5432/forge_op
server:   PostgreSQL 16.4 on x86_64-pc-linux-gnu
created:  forge_op
contents: empty; Forge will create its tables
usable:   yes

$ forge-server --config /etc/forge/forge.toml db check --json
{"created":false,"report":{"can_create":null,"contents":"forge","engine":"postgres","server_version":"PostgreSQL 16.4","tables":96},"usable":true}
```

### forge-server db show

Prints the operational database and the analytics settings, with any password
replaced by `***`. It takes no options.

**Sample output**

```text
$ forge-server --config /etc/forge/forge.toml db show
operational: postgres://forge:***@db1:5432/forge_op
analytics:   engine=Off database=forge_op_analytics
```

## forge-server enroll-token

Creates a node enrollment token on the server host, for the installer's
"agent on this host" option. It creates the cluster if the workspace has none
of that name. It prints only the token on standard output (logs go to
standard error), so a script can capture it. The token is filed under the
first platform administrator, and the audit log records
`server.enrollment_token.create` (and `cluster.create` when it made one) as
`system:forge`. Run it as root or the service user.

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--cluster <CLUSTER>` | string, 1-63 letters, digits, space, `-` `_` `.` | | **Yes** | Cluster the node joins; created if missing | `--cluster default` |
| `--hostname <HOSTNAME>` | string | | **Yes** | Expected host name, shown in the onboarding list until the node enrolls | `--hostname "$(hostname)"` |
| `--ttl-minutes <TTL_MINUTES>` | integer, minutes, 1-1440 | `15` | No | How long the token is valid | `--ttl-minutes 15` |

**Examples**

```sh
FORGE_ENROLL_TOKEN="$(sudo /opt/forge/forge-server --config /etc/forge/forge.toml \
  enroll-token --cluster default --hostname "$(hostname)" --ttl-minutes 15)"
```

:::caution
The token lets one node join the cluster. Pass it through the environment,
never on a command line, and unset it once the node has enrolled.
:::

## forge-server release

Release signing, for CI and maintainers. The private key never lives on a
Forge server; servers keep only the public key (`[update] public_key`).

```text
Usage: forge-server release [OPTIONS] <COMMAND>

Commands:
  keygen  New Ed25519 key: the private key (keep secret) and the public key
  sign    Signs a file (e.g. SHA256SUMS) with FORGE_RELEASE_KEY
  pubkey  Prints the public key of FORGE_RELEASE_KEY
```

### forge-server release keygen

Prints a new Ed25519 key pair. Store the private key as a CI secret.

```text
Usage: forge-server release keygen [OPTIONS]
```

```text
$ forge-server release keygen
private (FORGE_RELEASE_KEY, keep secret): <private-key>
public  (update::RELEASE_PUBLIC_KEY):     <public-key>
```

### forge-server release sign

Prints the base64 signature of `<FILE>` with the key in `FORGE_RELEASE_KEY`.

```text
Usage: forge-server release sign [OPTIONS] <FILE>
```

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<FILE>` | path | | Yes | The file to sign, normally `SHA256SUMS` | `dist/SHA256SUMS` |
| `FORGE_RELEASE_KEY` | env | | Yes | The private key from `keygen` | |

```sh
FORGE_RELEASE_KEY="$RELEASE_KEY" forge-server release sign dist/SHA256SUMS > dist/SHA256SUMS.sig
```

### forge-server release pubkey

Prints the public key of `FORGE_RELEASE_KEY`, to check which key CI signs
with.

```text
Usage: forge-server release pubkey [OPTIONS]
```

Without the key, `sign` and `pubkey` fail with exit code `1`:
`Error: set FORGE_RELEASE_KEY`.

## See also

- [Configuration](/docs/reference/configuration/): every `forge.toml` key and its `FORGE_*` variable
- [Server installer](/docs/reference/cli/server-installer/): installs, upgrades and runs `init`
- [Releases](/docs/development/releases/): how releases are built and signed
- [Security overview](/docs/security/)
