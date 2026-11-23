---
title: Security overview
description: How Forge protects a fleet, where it enforces each control and what is still open.
sidebar:
  order: 1
---

This page gives you the whole picture of how Forge protects your nodes in one
place. It links to the pages that explain each control in depth.

## The layers

Forge assumes the browser, the network and any single component can fail.
Each layer checks again instead of trusting the one before it.

| Layer | What protects it | Read more |
| --- | --- | --- |
| Identity | Argon2id passwords, login throttling, a self-hosted login challenge, MFA (TOTP and passkeys), step-up for sensitive actions, OIDC single sign-on | [Security model (identity)](/docs/concepts/security-model/), [MFA](/docs/guides/mfa/), [Passkeys](/docs/guides/passkeys/) |
| Authorization | Cedar policies checked at the API, in every job step, in the agent hub, on the agent and in the plugin host | [Authorization](/docs/concepts/authorization/), [Cedar schema](/docs/reference/cedar-schema/) |
| Audit | Every mutation and every denial is recorded in a hash-chained log | [Audit log](/docs/concepts/audit/) |
| Agent link | Agents dial out over mutual TLS with an internal CA and run only signed, short-lived, replay-protected commands | [Agents](/docs/concepts/agents/), [Agent protocol](/docs/reference/agent-protocol/) |
| Node | A root helper that accepts a fixed allowlist of operations; protected paths in the file explorer | [Agents](/docs/concepts/agents/), [File explorer](/docs/concepts/file-explorer/) |
| Extensions | Signed WebAssembly plugins with fuel, memory and time budgets | [Plugins](/docs/concepts/plugins/) |
| Secrets | Envelope-encrypted at rest, kept out of URLs and logs | [Credential vault](/docs/guides/credential-vault/) |
| Remote reach | Port tunnels to catalogued services over the API port, by environment role or a time-limited grant; the agent dials local addresses only | [Port tunnels](/docs/concepts/port-tunnels/) |

- [What Forge already enforces](/docs/security/hardening/) lists each control and
  what it does.
- [Open security work](/docs/security/roadmap/) lists the known gaps, what they
  could lead to and how they will be fixed.

## Sensitive actions

Some Cedar actions give root-level control of a node, for example
`server.command`, `terminal.open`, `file.write`, `stack.privileged` and
`secret.read`. A platform guardrail policy (`guard:sensitive-requires-mfa`)
forbids these actions unless the session is MFA-verified. A forbid overrides
every permit, so no role can bypass it. The full list is in the
[Cedar schema](/docs/reference/cedar-schema/). On an installation that does not
require MFA, sessions count as MFA-verified and the guardrail passes; see the
next section.

## MFA is your choice

MFA is each user's choice. A user who enrolls an authenticator app or a
passkey in **Profile** → **Password & security** is asked for it at every
sign-in, whatever the server setting.

Whether guarded actions also need MFA is a decision for each installation,
made by its operator. It does not depend on the `environment` setting, and
it is off by default. Different installations have different risks:

- A vendor-side or internal Forge, used by a small team on a private
  network, may leave MFA to each user.
- A client-facing or production Forge should require MFA.

You choose at install time with `install.sh --mfa required` or
`--mfa off` (the default on a new install; an upgrade keeps the current
setting). Later, `forge-server mfa on` or `forge-server mfa off` changes
`[security] require_mfa` in `forge.toml`, and a restart applies it. See
[Run Forge in production](/docs/guides/production-setup/#2-require-mfa-for-guarded-actions)
and the [CLI reference](/docs/reference/cli/).

When MFA is not required:

- Every browser session and API key counts as MFA-verified, so the
  `guard:sensitive-requires-mfa` guardrail no longer stops anyone, and
  revoking a node asks for no authenticator code. Cedar permissions still
  apply as before.
- AI connectors are not affected. Their sessions never count as
  MFA-verified, and the MCP endpoint refuses destructive and MFA-guarded
  tools. See [MCP connectors](/docs/concepts/mcp-connectors/).
- Users who enrolled MFA still need it to sign in.
- **Settings** → **Sign-in & MFA policy** shows **MFA on this server** as
  "each user's choice: protects their sign-in, not required for actions".
- The server logs an `MFA policy` info line at every start.

Each start that runs with a different setting than the last one writes
`config.mfa` to the audit log, with `old` and `new` (`required` or `off`) and
actor `system:forge`. You can see who could act without MFA, and since when.

## Sign-in protection

Password sign-in carries a login challenge: a small proof of work that the
browser solves before it sends the password. Forge issues and checks it
itself, with no third-party service, so it works on-premises and offline.
Each challenge costs a fraction of a second in a browser. Every failed
password for a sign-in name doubles the work of that name's next challenge,
up to 128 times. Failures also count per client address (not loopback;
behind a proxy, set `[security] trusted_proxies`), so trying many names from
one address gets slower too. The extra work ends 15 minutes after the last
failure; a successful sign-in ends it for the name, not for the address. Together with login throttling (10 failures in 15 minutes),
this makes guessing passwords slow and costly. Passkey and SSO sign-in do not
use the challenge. It is on by default (`[security] login_challenge`); see
the [configuration reference](/docs/reference/configuration/#security) and
[`GET /auth/challenge`](/docs/reference/rest-api/#login-challenge).

After 2 failed sign-ins for a sign-in name, or from one client address, the
next sign-in also shows a picture of 6 letters and digits to type. Forge
draws the picture itself as a PNG (no third-party service; the characters are
never sent as text), keeps only a keyed hash of the answer, and each picture
works once, for 2 minutes, from the address it was shown to (for IPv6, the
same /64 network, which also counts as one address). A wrong answer
shows a new picture; **New picture** gives another one. Failures count per
name and per address, so switching sign-in names does not skip it. Set the
number with `[security] captcha_after_failures` (`0` turns it off); see the
[configuration reference](/docs/reference/configuration/#security) and
[`GET /auth/captcha`](/docs/reference/rest-api/#image-captcha).

:::note
The picture has no audio alternative yet. Passkey sign-in never shows it,
so a passkey is the accessible way in; SSO sign-in does not show it either.
Otherwise the failures expire 15 minutes after the last one.
:::

:::caution
Pictures and failure counts live in the memory of one server process. With
several Forge server instances behind a load balancer, use sticky sessions
(client-address affinity): a picture fetched from one instance is refused by
another, and each instance counts failures on its own.
:::

When MFA is off, protection of sign-in rests on strong passwords (at least 12
characters), login throttling, the login challenge and the image CAPTCHA.

:::caution
Sensitive actions run as root on your nodes. Without MFA, one stolen password
gives an attacker root on every node that user can reach. Leave MFA to
each user only where you accept that risk, and run `forge-server mfa on` on
any Forge that clients or production systems depend on.
:::
