---
title: What Forge already enforces
description: The security controls built into Forge today, each with a plain explanation of what it stops.
sidebar:
  order: 2
---

This page lists the controls Forge enforces today. Each one is in the code
and covered by tests. For gaps that are still open, see
[Open security work](/docs/security/roadmap/).

## Authorization

- **Cedar at every enforcement point.** The API handler, every job step, the
  agent hub, the agent and the plugin host each check permission again. The
  UI only hides buttons; it never decides. See
  [Authorization](/docs/concepts/authorization/).
- **Job steps are re-authorized.** A long job cannot keep running on a
  permission that was removed after it started.
- **Guardrails win.** Platform `forbid` policies, such as MFA for sensitive
  actions, override every role.

## Audit

- **Every mutation is recorded, including denials.** You can see who tried
  what, not only what succeeded.
- **The log is hash-chained.** Removing, editing or reordering an entry breaks
  the chain. See [Audit log](/docs/concepts/audit/).

## Sessions and the web API

- **Cookies are HttpOnly and SameSite=Strict.** Scripts cannot read the
  session, and other sites cannot send it.
- **CSRF token on every unsafe request.** Every request other than `GET`,
  `HEAD` and `OPTIONS` must carry `X-CSRF-Token`, whatever its content type,
  including `text/plain`.
- **No CORS.** Forge sends no CORS headers, so other origins cannot read API
  responses.
- **Login throttling.** Failed sign-ins are limited per account and client
  address (10 in 15 minutes), per address (50) and per account (100), so one
  attacker can neither spray passwords nor lock the owner out. Step-up and
  inline code checks share a limit of 10. Behind a reverse proxy, Forge trusts
  `X-Forwarded-For` only from `[security] trusted_proxies`.
- **Single-use authenticator codes.** Each code works once per user.
- **Password changes end other sign-ins**, including app and connector
  tokens.
- **Refresh tokens expire.** 30 days after the original sign-in by default
  (`[security] refresh_token_days`), however often they are refreshed.
- **CLI device sign-in is never MFA-verified.** A code someone was tricked
  into approving gives no MFA-protected access, the approval page shows the
  requesting address and client, one address can hold at most five pending
  requests, and issuing tokens is audited.
- **Idempotent retries stay with the credential.** A stored answer is
  replayed only to the same person with the same kind of credential, never
  on sign-in, token or reveal paths, and each replay is audited.
- **SSO is tied to the browser.** The callback is accepted only in the browser
  that started the sign-in.
- **Password hashing off the request threads.** Argon2 runs on blocking
  threads, at most four at once.
- **Login challenge.** Every password sign-in carries a solved proof of work
  that Forge issues and checks itself, with no third-party service. A
  challenge is single use and expires after 2 minutes. Each failed password
  makes the next challenge for that sign-in name, and from that client
  address (not loopback; see `trusted_proxies` above), twice as hard, up to
  128 times, until 15 minutes pass without a failure. A successful sign-in
  resets the name, not the address. See
  [`GET /auth/challenge`](/docs/reference/rest-api/#login-challenge).
- **Image CAPTCHA.** After `[security] captcha_after_failures` failed
  sign-ins (default `2`) for a name or from an address (loopback included),
  the next sign-in also needs the characters from a server-drawn picture.
  Only an HMAC of the answer is stored; each picture is single use, bound to
  the client address (IPv6: its /64) and valid for 2 minutes. A full store
  drops its oldest pictures rather than refusing new ones, and every
  counter map is size-capped. Neither the answer nor the
  picture is logged. See
  [`GET /auth/captcha`](/docs/reference/rest-api/#image-captcha).
- **Body size cap.** API requests over 16 MiB are refused.
- **Security headers** on every response:

  | Header | Value |
  | --- | --- |
  | `Content-Security-Policy` | `default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'` |
  | `X-Frame-Options` | `DENY` |
  | `X-Content-Type-Options` | `nosniff` |
  | `Referrer-Policy` | `no-referrer` |
  | `Cross-Origin-Opener-Policy` | `same-origin` |
  | `Strict-Transport-Security` | `max-age=31536000`, only when `cookie_secure` is on (HTTPS) |

- **Paths only in request logs.** Query strings can carry codes and are not
  logged.

## Web console

- **No HTML from data.** The console never injects HTML: names, logs,
  approval requests and file contents are shown as text. The Markdown file
  preview builds its own elements and links only to `http`, `https` and
  `mailto`.
- **Links stay links.** A release-feed link is shown only when it is an
  `https` page, so a feed cannot hand the console a `javascript:` link.
- **No secrets in the browser.** Session tokens live only in HttpOnly
  cookies, never in `localStorage`, `sessionStorage` or a URL. Browser
  storage holds view preferences only.
- **Pages cannot be framed.** The strict content security policy,
  `frame-ancestors 'none'` and `X-Frame-Options: DENY` cover every page,
  including the consent, device sign-in and approval pages, so they cannot be
  overlaid in another site. The sign-in challenge runs in a same-origin
  worker, with no inline scripts or `eval`.
- **The console files are served safely.** The server answers only with the
  exact files built into it (no path from the request reaches the disk), with
  fixed content types; unknown API, agent and OAuth paths return `404`, never
  the console page.
- **Errors say what, not how.** Unexpected failures return a generic
  "internal error"; details stay in the server log.

## Credentials and cryptography

- **Tokens are stored only as hashes.** A copy of the database does not reveal
  session tokens or API keys.
- **Constant-time comparison** of secrets, so timing does not leak them.
- **Passwords use Argon2id**, with a minimum length of 12 characters.
- **Owner-only data on the server.** The data directory (`/var/lib/forge`)
  holds the database and its `-wal`/`-shm` files, the keys, the internal CA
  (`pki/`), database backups (`backups/`) and staged updates (`updates/`).
  At every start the server makes the directory and those subdirectories
  `0700` and takes group and other permissions off every file in them; new
  files follow the umask `077` (the unit also sets `UMask=0077`). If the data
  directory itself can't be restricted, the server doesn't start. A file it
  can't change is logged with its path and the fix (`chown forge:forge <path>
  && chmod go-rwx <path>`). On Windows the installer's ACL (SYSTEM and
  Administrators only, inherited) covers the same files. With PostgreSQL there
  is no database file, but the keys, CA, backups and updates are restricted
  the same way.
- **One crypto library: `ring`.** No OpenSSL anywhere in Forge.
- **No `unsafe` Rust.** It is denied workspace-wide. The Windows service
  module is the one documented exception.

## MFA

- **TOTP and passkeys.** Passkeys always require user verification. See
  [MFA](/docs/guides/mfa/) and [Passkeys](/docs/guides/passkeys/).
- **Step-up for sensitive actions.** When the server requires MFA, actions
  that give root on a node, such as `server.command`, `terminal.open` and
  `file.write`, need an MFA-verified session. If yours is not, Forge asks for
  a code first.
- **Require MFA in production.** By default MFA is each user's choice: it
  protects their sign-in, and no action asks for it. On a Forge that clients
  or production systems depend on, run
  `forge-server --config /etc/forge/forge.toml mfa on` and restart the
  server, or install with `install.sh --mfa required`. Guarded actions then
  need an MFA-verified session. Each change is recorded as `config.mfa` in
  the audit log. AI connectors never count as MFA-verified. See
  [MFA is your choice](/docs/security/#mfa-is-your-choice).

:::caution
While `[security] require_mfa` is off, a stolen password is enough to run
guarded actions, such as opening a root terminal, on any account that has
not enrolled MFA, and with an API key on any account. Accounts with MFA
enrolled still need their second factor to sign in.
:::

- **AI connector approvals are re-checked.** Approving a change an agent
  asked for needs MFA for destructive tools, and runs only while the
  connection, its connector, `[mcp] enabled` and the mode still allow it.
  Revoking a connection cancels its waiting requests, and every refused
  decision is audited. See [MCP: Approvals](/docs/reference/mcp/#approvals).

## Agents and nodes

- **Agents dial out.** Nodes need no inbound ports.
- **Pinned CA at enrolment.** The token carries the SHA-256 of the control
  plane's CA. The agent refuses a CA download that holds any other or any
  extra certificate, and trusts only the pinned one.
- **Mutual TLS with an internal CA.** Each agent has its own short-lived
  certificate, renewed automatically. Only the node's current certificate
  can renew, and only when renewal is due; the replaced certificate is
  revoked at once. A revoked node's next request is refused.
- **Results only from the node that was asked.** Forge accepts a command's
  output and result only from the node it sent the command to.
- **Signed commands.** Each command is signed, expires within 5 minutes and
  carries an operation id. The agent refuses replays and commands issued
  before it started.
- **The agent checks again.** It runs only command kinds in its local
  allowlist in `agent.toml`.
- **The root helper trusts only Forge.** It verifies each command's
  signature against a key pinned in its root-only folder, refuses replays and
  works out what to run from the command itself, with a fixed allowlist and
  no shell. Code running as the agent user cannot act as root. See
  [Agents](/docs/concepts/agents/#privileged-helper).
- **Verified self-update with rollback.** The helper checks the SHA-256 of the
  new binaries from the signed release before swapping them. It stages them
  in a folder only root can open, so nothing can swap a file between the
  check and the install. If the new agent does not come back, the node rolls
  back on its own. A node gets one update at a time, and one rollout runs at
  a time. Automatic rollouts run as the `platform_admin` who saved the
  setting, only while that person is enabled and for 90 days after their
  MFA-verified save; every refusal is audited. See
  [Update agents](/docs/guides/update-agents/).
- **No token on a command line.** The node installer passes the enrolment
  token to the agent through its environment, so `ps` does not show it.
- **App launchers ignore planted links.** The helper refuses an app's `.forge`
  folder unless it is a real folder owned by root. See
  [Services](/docs/guides/services/).

## Compose stacks and containers

- **Host access is explicit.** Privileged settings, host networking, host
  environments and binds of system paths need `stack.privileged`. Service keys
  are an allowlist.
- **Binds stay where they point.** The node walks each bind path without
  following links, and a bind inside another writable bind is refused.
- **`.env` values stay sealed.** Reads, previews, findings and audit details
  show `${VAR}`, never the value.

## Sites, edges and terminals

- **Sites stay in their workspace.** A site is listed, read, changed and
  applied only within its own workspace.
- **Custom Nginx blocks use an allowlist.** Only directives from a fixed list
  are accepted: no file paths, process settings or code. Saving or applying a
  non-empty block needs `server.command` (MFA) on every edge. See
  [Sites and DNS](/docs/guides/ingress/#custom-configuration).
- **Uploaded certificate keys are guarded.** Deploying one needs
  `certificate.use` (administrators, MFA), and the key goes only to the edges
  of the site being applied, checked again when the job runs.
- **Terminal certificates are bound to one node.** Each certificate names
  the node it is for, and a Linux node refuses certificates for other nodes.
- **Operator restrictions survive updates.** Command names removed from an
  agent's `allowed_commands` stay removed when the installer runs again.
- **SSH endpoints cannot point elsewhere.** Changing a node's SSH endpoint or
  host key needs `server.command` (MFA), and Forge refuses an endpoint or host
  key that belongs to another node.
- **Terminals do not outlive access.** A terminal closes when the sign-in or
  the `terminal.open` permission is gone, and after 12 hours at most.
- **Moving a node needs rights on both sides.** `server.enroll` on the cluster
  or environment it leaves and on the one it joins.

## Plugins

- **Sandboxed in WebAssembly** with fuel, memory and time budgets.
- **Signed manifests.** Forge loads only plugins signed by a trusted
  publisher. A plugin id always starts with its publisher, so one publisher
  cannot install or upgrade another's plugin.
- **The runner checks what it runs.** It compares the artifact it loads with
  the installed SHA-256, and the memory budget covers all of a plugin's
  memories together.
- **Audited, including refusals.** Every install, change and invocation is
  recorded, and so is every refused attempt.
- **Least privilege.** A plugin's effective permission is the user's
  permission intersected with the plugin's and the policy's. See
  [Plugins](/docs/concepts/plugins/).

## Secrets

- **Sealed at rest.** Secrets are envelope-encrypted with the server's key.
- **Kept out of URLs and logs.**
- **Not returned by default.** System secrets are never shown. Revealing a
  vault password needs a platform administrator, `secret.read` and an
  MFA-verified browser session. See [Credential vault](/docs/guides/credential-vault/).

## File explorer

- **Confined paths.** Every path is resolved on the node. A symlink that leads
  out of an approved root is refused.
- **Protected paths.** Files such as `/etc/shadow`, `/etc/gshadow`, SSH keys
  and Forge's own data are refused, even inside an approved root. See
  [File explorer](/docs/concepts/file-explorer/).
- **Protected folders stay whole.** A move or delete is refused when anything
  protected could lie below it, so `.ssh` folders and Forge's own data cannot
  be carried or deleted along with a parent.
- **Short names and setuid.** Windows 8.3 short names are refused, and
  replacing a file clears setuid and setgid.
- **Archives belong to their creator.** A download archive can only be read or
  deleted by the user who made it, through the same root.
- **SFTP sessions are counted at login.** The session limit holds across
  parallel logins and extra channels.
- **App folders are worked on as the app (Linux).** Every file explorer and
  SFTP operation on a native app's folder runs on the node as that app's own
  account, in a separate worker process, never as root. A bug in Forge's
  path checks there reaches no more than the app itself could. Roots around
  or inside app folders are refused, owners cannot be changed and setuid and
  setgid bits are removed. See [App roots](/docs/concepts/file-explorer/#app-roots).

:::caution[Set `kernel.yama.ptrace_scope` to 1 on Linux nodes]
An app's own processes share the account its file worker runs as. With
`kernel.yama.ptrace_scope = 0`, they can attach to a running worker with
`ptrace`. With `1`, only a parent process or root can attach, which closes
this. Forge does not change the setting for you.

| Distribution | Default |
| --- | --- |
| Debian | `0`: set it to `1` |
| Ubuntu | `1`: nothing to do |
| RHEL, Rocky Linux, AlmaLinux, Fedora | `0`: set it to `1` |

Check the current value, then set it and keep it across reboots:

```sh
sysctl kernel.yama.ptrace_scope
echo 'kernel.yama.ptrace_scope = 1' | sudo tee /etc/sysctl.d/60-ptrace.conf
sudo sysctl --system
```

With `1`, users other than root can no longer attach a debugger such as
`gdb -p` or `strace -p` to a running process unless it is their own child
process.
:::

## Port tunnels

- **Service targets only.** A tunnel reaches a port only when the service
  catalogue lists it for the named service, stack service, database or
  SSH. Raw ports need `network.tunnel.any` and are audited as raw.
- **Local dial only.** The agent connects to loopback, its own addresses or
  its cluster-network address, never to another machine.
- **Closes when in doubt.** Every 15 seconds the server re-checks the
  sign-in, the role or grant and the expiry; a failed check ends every
  connection within 30 seconds. A server restart records open tunnels as
  closed. See [Port tunnels](/docs/concepts/port-tunnels/).
