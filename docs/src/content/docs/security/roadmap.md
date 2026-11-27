---
title: Open security work
description: Known security gaps in Forge, what each could lead to, and the planned fix and status.
sidebar:
  order: 3
---

This page lists security issues found in the ongoing review and the
improvements we plan. Each item says what could happen, why it matters, what
we will do and where the work stands. Use it to judge your own risk until a
fix ships.

:::note
When an item is fixed, it moves to [What Forge already enforces](/docs/security/hardening/).
An item marked **Accepted** is a deliberate design choice, not a pending
fix. It stays on this page so you can judge it for your own machines.
:::

## Summary

| Item | Area | Status |
| --- | --- | --- |
| [MFA enrolment without re-authentication](#mfa-enrolment-without-re-authentication) | Identity | Deferred |
| [Windows agent runs as LocalSystem](#windows-agent-runs-as-localsystem) | Agents | Accepted |
| [Changing a password leaves other sign-ins open](#changing-a-password-leaves-other-sign-ins-open) | Identity | Fixed |
| [An authenticator code can be used twice](#an-authenticator-code-can-be-used-twice) | Identity | Fixed |
| [App refresh tokens never expire](#app-refresh-tokens-never-expire) | Identity | Fixed |
| [Sign-in throttling is per account only](#sign-in-throttling-is-per-account-only) | Identity | Fixed |
| [SSO sign-in is not tied to the browser](#sso-sign-in-is-not-tied-to-the-browser) | Identity | Fixed |
| [Enrolment CA bundle trusts extra certificates](#enrolment-ca-bundle-trusts-extra-certificates) | Agents | Fixed |
| [Symlinks in a native app's `.forge` folder](#symlinks-in-a-native-apps-forge-folder) | Apps | Fixed |
| [Container-app volumes skip the host-mount policy](#container-app-volumes-skip-the-host-mount-policy) | Apps | Fixed |
| [Container-app mounts outside the app folder need no MFA](#container-app-mounts-outside-the-app-folder-need-no-mfa) | Apps | Fixed |
| [Approval for deploys to production environments](#approval-for-deploys-to-production-environments) | Apps | Planned |
| [Sites are not scoped to a workspace](#sites-are-not-scoped-to-a-workspace) | Sites | Fixed |
| [Custom nginx blocks can write files as root](#custom-nginx-blocks-can-write-files-as-root) | Sites | Fixed |
| [Certificate private keys can go to any node](#certificate-private-keys-can-go-to-any-node) | Sites | Fixed |
| [Stack preview shows `.env` secrets](#stack-preview-shows-env-secrets) | Stacks | Fixed |
| [Stack bind mounts can escape through a symlink](#stack-bind-mounts-can-escape-through-a-symlink) | Stacks | Fixed |
| [`env_file`, `configs` and `secrets` can follow file links](#env_file-configs-and-secrets-can-follow-file-links) | Stacks | Fixed |
| [Host networking is not flagged as privileged](#host-networking-is-not-flagged-as-privileged) | Stacks, containers | Fixed |
| [Gaps in the compose denylist](#gaps-in-the-compose-denylist) | Stacks | Fixed |
| [Stack preview skips the workspace check](#stack-preview-skips-the-workspace-check) | Stacks | Fixed |
| [SSH endpoint redirect gives a root shell on another node](#ssh-endpoint-redirect-gives-a-root-shell-on-another-node) | Terminal, nodes | Fixed |
| [A node can be moved into another workspace's environment](#a-node-can-be-moved-into-another-workspaces-environment) | Nodes, workspaces | Fixed |
| [Helper staging race](#helper-staging-race) | Agents | Fixed |
| [Windows protected paths can be bypassed](#windows-protected-paths-can-be-bypassed) | File explorer | Fixed |
| [Renaming or deleting a parent folder bypasses protected paths](#renaming-or-deleting-a-parent-folder-bypasses-protected-paths) | File explorer | Fixed |
| [setuid bit kept when a file is overwritten](#setuid-bit-kept-when-a-file-is-overwritten) | File explorer | Fixed |
| [Enrolment token visible in the process list](#enrolment-token-visible-in-the-process-list) | Agents | Fixed |
| [SFTP sessions are not limited](#sftp-sessions-are-not-limited) | SFTP | Fixed |
| [An update re-allowed commands the operator removed](#an-update-re-allowed-commands-the-operator-removed) | Agents | Fixed |
| [Login challenge against brute force](#login-challenge-against-brute-force) | Identity | Fixed |
| [MFA as the operator's choice](#mfa-as-the-operators-choice) | Identity | Fixed |
| [SSH certificates bound to one node](#ssh-certificates-bound-to-one-node) | Terminal | Fixed |
| [The root helper checks Forge's signature itself](#the-root-helper-checks-forges-signature-itself) | Agents | Fixed |

## MFA enrolment without re-authentication

**What could happen:** An attacker steals the session of a user who has no
MFA yet. With that session alone, they enrol their own authenticator. They
now have an MFA-verified session and can run sensitive actions.

**Why it matters:** Sensitive actions run as root on nodes. MFA should prove
the person, not only the session. This matters only on installations that
use MFA.

**What we will do:** Require the current password to enrol or replace an
authenticator, and the current code too when MFA is already on.

**Status:** Deferred.

## Windows agent runs as LocalSystem

**What could happen:** On Windows, the `ForgeAgent` service runs as
LocalSystem and does privileged work in its own process. A flaw in the agent,
or anyone who gets code running inside it, has full control of the machine:
every file, service and account on it.

**Why it matters:** On Linux the agent runs as the unprivileged
`forge-agent` user, and only a small root helper acts, after it checks Forge's
signature on each command itself. On Windows there is no such split, so the
agent's whole network-facing code runs with the highest privilege on the
node.

**Why Forge keeps it:** The agent's job on Windows is administrative:
services, packages, files, processes and accounts. A split into an
unprivileged agent and a privileged helper service would add a second
service and a named-pipe protocol, and the helper would still need
LocalSystem for that same work. The risk is limited instead:

- The agent opens no listening port. It connects out to the control plane
  over mutual TLS, so nothing on the network can talk to it directly.
- It runs only commands the control plane signed, and checks each
  signature, its expiry and its node before acting.
- Every action needs a Cedar permission in Forge, the root-equivalent ones
  an MFA-verified session where MFA is required, and each one is audited.
- `allowed_commands` limits what the agent accepts on each machine (see
  [Restrict what an agent may do](/docs/guides/add-nodes/#restrict-what-an-agent-may-do)).
- Its binary and data folders are writable by SYSTEM and Administrators
  only.

**Status:** Accepted as a design decision. Add Windows machines only where
you accept that the agent has full control of them, and restrict its
`allowed_commands` where you need less.

## Changing a password leaves other sign-ins open

**What could happen:** You change your password because you suspect someone
has your session. Their browser session and app refresh tokens keep working.

**Why it matters:** Changing the password is the first thing people do after a
suspected compromise, and it does not end the attacker's access.

**What we will do:** End every other sign-in of the user, including app
refresh tokens, when the password changes. The session that made the change
stays signed in.

**Status:** Fixed. A password change ends every other sign-in, including app and connector tokens; the browser that made it stays signed in.
## An authenticator code can be used twice

**What could happen:** A six-digit code that someone saw once, for example
over a shoulder or through a phishing page, still works for about a minute
and a half. The code check on node revocation also has no attempt limit.

**Why it matters:** A seen or guessed code turns into a verified session for
sensitive actions.

**What we will do:** Accept each code only once per user, and throttle every
endpoint that checks a code.

**Status:** Fixed. Each code works once per user, and every endpoint that checks a code shares the step-up limit.
## App refresh tokens never expire

**What could happen:** An app refresh token gets a new lifetime every time it
is used, and keeps its MFA status. A stolen token used once a month works
forever.

**Why it matters:** A single leaked token becomes permanent access.

**What we will do:** Give refresh tokens an absolute lifetime counted from the
original sign-in, set in the configuration.

**Status:** Fixed. Refresh tokens expire `[security] refresh_token_days` (30 by default) after the original sign-in.
## Sign-in throttling is per account only

**What could happen:** An attacker tries one password on many accounts, which
the per-account limit does not catch. Repeated failures for one account also
lock out its real owner, and password checks can slow the whole server.

**Why it matters:** Password spraying works, and a single client can deny
sign-in to an administrator.

**What we will do:** Add a limit per client address, trusting proxy headers
only from configured proxies, and run password hashing off the request
threads.

**Status:** Fixed. Limits per account and address, per address and per account; `X-Forwarded-For` only from `[security] trusted_proxies`; Argon2 on blocking threads. See the [upgrade note](/docs/reference/upgrade-notes/#sign-in-hardening) for proxies.
## SSO sign-in is not tied to the browser

**What could happen:** An attacker starts an SSO sign-in with their own account
and sends the callback link to someone else. That person ends up signed in to
Forge as the attacker.

**Why it matters:** The victim may then upload files or secrets into the
attacker's account without noticing.

**What we will do:** Store the sign-in state in a short-lived cookie and accept
the callback only in the browser that started it.

**Status:** Fixed. A short-lived cookie set when the sign-in starts must match at the callback.
## Enrolment CA bundle trusts extra certificates

**What could happen:** When an agent enrols, it checks the pinned fingerprint
against the first certificate in the CA bundle only, but then trusts every
certificate in it. Someone who can tamper with the bundle in transit could
add their own CA after the real one, and the agent would trust it.

**Why it matters:** A trusted rogue CA lets an attacker impersonate the
control plane to the agent.

**What we will do:** Pin and trust exactly one CA.

**Status:** Fixed. The agent refuses a CA response that holds more than one
certificate, and trusts and saves only the pinned one.

## Symlinks in a native app's `.forge` folder

**What could happen:** A native app's files include a symlink in its `.forge`
folder. The install and remove steps run as root and follow the link, so
they write or delete files outside the app, anywhere on the node.

**Why it matters:** A crafted app package becomes a way to change any file on
the node as root.

**What we will do:** Refuse symlinks and resolve every path before acting on
it.

**Status:** Fixed. The helper opens `.forge` without following links, refuses
it unless it is a real folder owned by root, and writes the launcher and
runtime links relative to that open folder.

## Container-app volumes skip the host-mount policy

**What could happen:** Compose stacks check volume mounts against a host-mount
policy, but container apps do not. An app can mount the host's `/` or the
Docker socket into its container.

**Why it matters:** Either mount gives the container root on the node.

**What we will do:** Apply the same host-mount policy to apps as to stacks.

**Status:** Fixed. Container app volumes are refused on Docker sockets, system paths and Forge's own folders, on the server and again on the node after resolving links. Verified on the node with the next lab build.
## Container-app mounts outside the app folder need no MFA

**What could happen:** The host-mount policy for container apps refuses
system paths, but anyone with `deploy` (a **Developer**, an API key or a
connector, none of them with MFA) can mount other host folders writable,
such as `/var/spool/cron`, `/var/lib/dpkg` or `/home`, or another
environment's app folder. Compose stacks already allow only their own folder
and approved file roots.

**Why it matters:** The container usually runs as root, so a root crontab,
a package script or a user's `authorized_keys` written through the mount
gives root on the node, and another app's folder gives its data.

**What we will do:** Apply the stacks' rule to apps: only the app's own
folder and approved file roots for a plain deploy; anything else needs
`stack.privileged` with MFA.

**Status:** Fixed. The deploy API checks each node's mounts against its
approved roots and asks for `stack.privileged`; the job re-authorizes it at
every step and refuses a mount added after submission. System paths stay
refused for everyone. `deploy` itself stays outside the MFA guardrail so CI
pipelines with API keys keep working.

## Approval for deploys to production environments

**What could happen:** `deploy` needs no MFA, so a stolen browser session or
API key can ship any unprivileged workload to an environment.

**Why it matters:** Unprivileged workloads still run code on your nodes and
reach their networks.

**What we will do:** A per-environment setting that sends deploys through the
approval queue, which works for CI pipelines and connectors where MFA
cannot.

**Status:** Planned.

## Sites are not scoped to a workspace

**What could happen:** Sites do not belong to a workspace. A user in one
workspace can read or edit the sites of another workspace.

**Why it matters:** Workspaces are meant to separate teams or clients.
This breaks that separation for sites.

**What we will do:** Scope sites to a workspace and authorize every site
route against it.

**Status:** Fixed. Sites are listed, read, changed and applied only within their own workspace, and a cluster from another workspace is refused.
## Custom nginx blocks can write files as root

**What could happen:** A site's custom nginx block accepts raw directives.
nginx runs them as root on the proxy node, so a directive such as an
`access_log` path into `/etc/cron.d` writes a file there.

**Why it matters:** Writing into `/etc/cron.d` runs commands as root. A user
who can edit a site gets root on the proxy node.

**What we will do:** Allow only a list of known directives and forbid
directives that take a file path.

**Status:** Fixed. Custom blocks accept only directives from a fixed list, with no file paths, process settings or code. Saving or applying one needs `server.command` (MFA) on every edge. See the [upgrade note](/docs/reference/upgrade-notes/#custom-nginx-blocks-supported-directives-only).
## Certificate private keys can go to any node

**What could happen:** Anyone with `proxy.manage` can use a site command to
copy an uploaded certificate's private key to any node, not only the nodes
that serve the site.

**Why it matters:** A private key on the wrong node can be read there and used
to impersonate your sites.

**What we will do:** Send keys only to the nodes the site is placed on, and
require a separate permission to move them.

**Status:** Fixed. Deploying an uploaded certificate needs the new `certificate.use` permission (administrators, MFA). The key goes only to the edges of the site being applied, checked again by the job, and the generic command endpoint refuses uploaded certificates.
## Stack preview shows `.env` secrets

**What could happen:** The stack preview interpolates `.env` values into
fields such as `image`, `healthcheck` and `volumes`. A user with view-only
access sees the secret values there.

**Why it matters:** View-only users should not learn passwords or keys.

**What we will do:** Mask secret values in previews.

**Status:** Fixed. Stack reads, previews, findings, plan errors and audit details show `${VAR}` instead of `.env` values.
## Stack bind mounts can escape through a symlink

**What could happen:** One service in a stack creates a symlink to `/` inside the stack folder. A second service then bind-mounts that path, which the policy allowed because it looked like it was inside the stack folder.

**Why it matters:** The second container gets the node's whole file system, which is root on the node. This is a high-severity issue.

**What we will do:** Resolve symlinks before checking each bind source, and refuse sources that resolve outside the stack folder.

**Status:** Fixed. The node walks every bind path without following links and creates missing folders itself; a bind inside another writable bind of the stack is refused. Verified on the node with the next lab build. A link swapped in from outside the stack after the check remains possible; see [Compose stacks](/docs/concepts/compose-stacks/).
## `env_file`, `configs` and `secrets` can follow file links

**What could happen:** A stack's `env_file`, `configs` or `secrets` entry points at a file inside the stack folder that is a link to a file elsewhere, such as a key on the node.

**Why it matters:** The linked file's contents end up in a container, where the stack's users can read them.

**What we will do:** Resolve every referenced file and refuse links that lead outside the stack folder.

**Status:** Fixed. `env_file`, `label_file` and config and secret files follow the same host-path and nesting rules as binds.
## Host networking is not flagged as privileged

**What could happen:** A stack can join the host network through a network declared with `name: host`, and **Create container** accepts network `host`. Neither needs `stack.privileged`.

**Why it matters:** A container on the host network can reach services that listen only on the node's loopback address, and bind any port on the node.

**What we will do:** Treat host networking in every form as privileged and require `stack.privileged`.

**Status:** Fixed. A network named `host`, `container:` modes and other stacks' `service:` modes need `stack.privileged`, also in **Create container**.
## Gaps in the compose denylist

**What could happen:** Some compose features that run commands or share environments are not on the denylist: `post_start` and `pre_stop` hooks, `ipc` and `pid` set to `container:<name>`, and `provider` services.

**Why it matters:** These let a stack run commands outside the normal container start, see another container's processes, or start services that Forge does not check.

**What we will do:** Add these features to the denylist, or require `stack.privileged` for them.

**Status:** Fixed. Service keys are an allowlist; `provider:` is refused; privileged hooks, `use_api_socket` and `container:` environments need `stack.privileged`.
## Stack preview skips the workspace check

**What could happen:** The stack preview endpoint does not check that the stack belongs to your workspace. A user in one workspace can preview a stack definition they reference from another. Low severity.

**Why it matters:** Workspaces are meant to keep teams apart, even for read-only views.

**What we will do:** Check the workspace on the preview route like on every other stack route.

**Status:** Fixed. The preview refuses a node of another workspace and needs `server.manage` on the node.
## SSH endpoint redirect gives a root shell on another node

**What could happen:** A user who can change a node's SSH endpoint points it at a different node. When someone with access to the first node opens a terminal, the session lands as root on the second node. High severity.

**Why it matters:** Permission on one node turns into root on a node the user was never granted.

**What we will do:** Require MFA to change an SSH endpoint, refuse endpoints and host keys that belong to another node, and limit how long a terminal stays open.

**Status:** Fixed. Changing a node's SSH endpoint or host key needs `server.command` (MFA). Forge refuses an endpoint, address or host key that belongs to another node, and a host key the node's own agent does not report. Terminals end after 12 hours and close when the sign-in or the permission is gone. Binding SSH certificates to one node is planned below.
## A node can be moved into another workspace's environment

**What could happen:** A user moves a node they manage into an environment that belongs to a different workspace.

**Why it matters:** The other workspace now runs workloads on a node it does not own, or the user gains reach into that workspace.

**What we will do:** Allow a move only into environments of the same workspace, and authorize it on both sides.

**Status:** Fixed. Moving a node needs `server.enroll` on the cluster or environment it leaves and on the one it joins, also when onboarding into an environment.
## Helper staging race

**What could happen:** The root helper checks a staged file and then uses it. In the short window between the two, a local process can swap the file for another one (a time-of-check to time-of-use race).

**Why it matters:** The helper, running as root, then acts on a file the attacker chose.

**What we will do:** Stage into a directory only root can write, open the file once and act on that open handle.

**Status:** Fixed. Self-update downloads, checks, test-runs and installs from
`/var/lib/forge-agent-helper/update-staging`, a folder only root can open.
The pending marker that drives rollback lives there too; the agent's copy is
written without following links.

## Windows protected paths can be bypassed

**What could happen:** On Windows, the protected-path list misses some places, such as `C:\ProgramData`, and paths written as 8.3 short names (for example `PROGRA~1`) do not match the list.

**Why it matters:** Files that should be refused, such as service secrets, can be read or changed through the file explorer.

**What we will do:** Protect the missing locations and compare paths only after expanding them to their full long form.

**Status:** Fixed for 8.3 short names (`PROGRA~3`) and the agent's `ProgramData\OrbynForge` folder: both are refused. Still open on Windows: checking the final path of an opened file and detecting hard links, which need APIs Forge cannot use without `unsafe` code.
## Renaming or deleting a parent folder bypasses protected paths

**What could happen:** Forge refuses changes to a protected file, but not to its parent folder. Renaming or deleting the folder moves or removes the protected file with it.

**Why it matters:** A user can remove or replace files such as SSH keys that the file explorer is meant to keep safe.

**What we will do:** Refuse rename and delete on any folder that contains a protected path.

**Status:** Fixed. A move or delete is refused when anything protected could lie below it, such as `/home/<user>`, `/root`, `/etc`, `/var` or `ProgramData`, on the server and again on the node.
## setuid bit kept when a file is overwritten

**What could happen:** A user overwrites a program that has the setuid bit. The new content keeps the bit, so it runs as the file's owner, often root.

**Why it matters:** Anyone who can write the file and run it gets root on the node.

**What we will do:** Clear the setuid and setgid bits whenever a file's content is replaced.

**Status:** Fixed. Replacing a file through an upload always clears setuid and setgid; an administrator can set them again under **Permissions**.
## Enrolment token visible in the process list

**What could happen:** The enrolment token is passed on the agent's command line during installation. Any local user can read it with `ps` while the command runs.

**Why it matters:** With the token, another machine can enrol as a node in your fleet.

**What we will do:** Pass the token through an environment variable or a file readable only by root.

**Status:** Fixed. The installer exports the token to the agent's environment
and never puts it on a command line.

## SFTP sessions are not limited

**What could happen:** A user opens SFTP sessions without limit.

**Why it matters:** Many open sessions exhaust memory or file handles on the server, and other users lose access.

**What we will do:** Limit sessions per user and in total.

**Status:** Fixed. A login reserves its session slot as it succeeds, and each extra channel takes another slot.
## Login challenge against brute force

**What could happen:** An attacker spreads password guesses over many
accounts or addresses to stay under the throttling limit.

**Why it matters:** This matters most on installations that run without MFA,
where the password is the only factor.

**What we will do:** Add a self-hosted proof-of-work challenge to sign-in, on
top of the existing throttling. It uses no third-party service, so it works
on-premises and offline.

**Status:** Fixed. Every password sign-in needs a solved, single-use
challenge that expires after 2 minutes. Each failed password makes the next
challenge for that sign-in name twice as hard, up to 128 times, on top of the
throttle. See [What Forge already enforces](/docs/security/hardening/#sessions-and-the-web-api).

## MFA as the operator's choice

**What could happen:** The MFA guardrail applied to every installation.
Operators of internal installations could not opt out, and the policy could
not be set per installation.

**Why it matters:** Each operator should decide the trade-off for their own
installation.

**What we will do:** Choose at install time whether MFA is required, and
switch it later with a command on the server. See
[MFA is your choice](/docs/security/#mfa-is-your-choice).

**Status:** Fixed. `install.sh --mfa required|off` and `forge-server mfa on|off`
set `[security] require_mfa`, in any environment. Running without MFA shows a
console banner, logs a warning at start (an error in `production`) and
records each change as `config.mfa` in the audit log. See
[What Forge already enforces](/docs/security/hardening/#mfa).

## SSH certificates bound to one node

**What could happen:** A terminal certificate names only the login, such as
`root`, not the node it was issued for. If a terminal could still be steered
to another node that trusts Forge's SSH CA, the certificate would work there
too.

**Why it matters:** The checks on SSH endpoints close the known path, but the
certificate itself should only open the node it was issued for.

**What we will do:** Add a per-node principal to each certificate and have the
installer configure `AuthorizedPrincipalsFile`, so a certificate for one node
is refused by every other node.

**Status:** Fixed. Terminal certificates carry `forge.<server-id>.<login>` and each Linux node accepts only its own id, set up by the root helper after the agent update. See [Terminal](/docs/guides/terminal/#how-it-stays-safe).
## The root helper checks Forge's signature itself

**What could happen:** The root helper trusts every request from the
`forge-agent` user. Code running as that user could ask the helper to install
an update or write a file that Forge never signed.

**Why it matters:** The split between the agent user and root only holds if
the helper checks requests on its own.

**What we will do:** Pass the signed command to the helper and have it check
the signature, target node, expiry and replay with a key pinned at
enrollment in a root-only file, before it acts.

**Status:** Fixed. The agent forwards each command exactly as Forge signed it; the helper verifies it against a key pinned in its root-only folder, refuses replays and works out the request itself. Nodes enrolled before this release are pinned on first use from a file the agent user owns, so a node compromised before its update needs to be revoked and added again for full protection. See [Agents](/docs/concepts/agents/#privileged-helper).

## An update re-allowed commands the operator removed

**What could happen:** Re-running the node installer added back every command
name missing from the agent's `allowed_commands`, including ones the operator
had removed on purpose.

**Why it matters:** A restriction made on the node quietly disappeared at the
next update.

**What we will do:** Remember which command names were offered, and add only
names that are new in a release.

**Status:** Fixed. See the [upgrade note](/docs/reference/upgrade-notes/#agent-command-allowlist-removals-stay-removed).
