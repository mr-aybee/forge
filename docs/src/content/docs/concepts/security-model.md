---
title: Security model
description: Identity, secrets, containment and platform hygiene.
sidebar:
  order: 7
---

## Identity

- Passwords: Argon2id (m = 19 MiB, t = 2), minimum 12 characters, constant-time
  login for unknown users, throttling after 10 failures per 15 minutes.
- MFA: TOTP (RFC 6238) or a passkey. TOTP secrets are envelope-encrypted and
  bound to the user.
- Sessions: random 256-bit tokens; only SHA-256 hashes are stored.
- Web: `forge_session` cookie is HttpOnly, Secure, SameSite=Strict. A readable
  `forge_csrf` cookie must be echoed in `X-CSRF-Token` on mutations.
- Apps: OAuth 2.1 authorization code + PKCE (S256 only), exact or loopback
  redirect matching, single-use 60-second codes, rotating refresh tokens.
- SSO: OpenID Connect with PKCE, nonce and server-side state. Users are linked
  by subject or verified email; auto-provisioning is off by default. SSO
  sessions are not treated as MFA-verified unless proven.
- Passkeys: WebAuthn, verified by Forge itself with `ring`. A passkey always
  requires user verification, so a passkey sign-in or step-up is an
  MFA-verified session. See below.

## How Forge verifies passkeys

Forge is the WebAuthn relying party. It verifies registrations and sign-ins
itself, with `ring` and a small CBOR reader, instead of a WebAuthn library:
the common Rust library needs OpenSSL, and Forge uses one crypto library
only. The scope is kept small on purpose, because every line of it is
security-critical. Decision D-055 in `DECISIONS.md` records the reasoning.

```diagram
alt: Passkey sign-in sequence
sequence:
  actors: [Browser, Forge, Database]
  messages:
    - { from: Browser, to: Forge, label: "POST …/begin" }
    - { from: Forge, to: Database, label: "32 random bytes → SHA-256" }
    - { at: Database, note: "challenge hash, purpose, user, expiry (+5 min)" }
    - { from: Forge, to: Browser, label: "options (challenge, RP ID, UV required)" }
    - { at: Browser, note: "authenticator: PIN/biometric, signs with the private key" }
    - { from: Browser, to: Forge, label: "POST …/finish" }
    - { from: Database, to: Forge, label: "take challenge (deleted, used once)" }
    - { at: Forge, note: "check client data, flags, signature, counter" }
    - { from: Forge, to: Database, label: "public key, counter" }
    - { from: Forge, to: Browser, label: "session / 401" }
```

### What Forge accepts

| Aspect | Accepted | Not accepted |
| --- | --- | --- |
| Attestation | `none` with an empty statement | Every other format (`packed`, `tpm`, `android-key`, `fido-u2f`, `apple` …) |
| Algorithms | ES256 (COSE `-7`, P-256), EdDSA (`-8`, Ed25519), RS256 (`-257`, 2048 to 4096-bit RSA) | Every other algorithm or curve |
| Credentials | Discoverable (resident) credentials, up to 20 per user | Credential ids longer than 1023 bytes |
| User verification | Required at registration, sign-in and step-up | Responses without the UV flag |

Forge asks the browser for `none` attestation, which every browser supports.
The consequence: Forge cannot tell which make or model of authenticator
holds a passkey, so it cannot restrict users to, for example, hardware keys
from one vendor.

### What Forge checks

Each flow (register, sign-in, step-up) is two calls: `begin` creates a
challenge and `finish` verifies the browser's answer.

- **Challenge.** 32 random bytes from the system CSPRNG. Forge stores only
  its SHA-256 hash, with its purpose (`register`, `login` or `step_up`), the
  user (for register and step-up) and an expiry 5 minutes ahead. `finish`
  deletes the challenge whatever the outcome, so it works once. A challenge
  for another purpose, another user, or past its expiry is refused. The
  challenge in the client data is compared in constant time.
- **Client data.** The type is `webauthn.create` for registration and
  `webauthn.get` for sign-in and step-up. The origin must equal the origin of
  the request exactly (scheme, host and port: the address the browser is
  on), and cross-origin requests are refused.
- **Relying party.** The RP ID is the host of the request (from `Host`, or
  `X-Forwarded-Host` from a trusted proxy), on an allowed host. The SHA-256 of
  the RP ID must match the hash in the authenticator data.
- **Flags.** User present (UP) and user verified (UV) must both be set.
  Registration must carry attested credential data; a sign-in must not.
- **Signature.** Forge verifies the signature over the authenticator data
  and the hash of the client data, with the public key stored at
  registration.
- **Signature counter.** If the authenticator counts, the new counter must
  be higher than the stored one; otherwise Forge refuses the sign-in as a
  possible cloned key. Synced passkeys report `0` every time; Forge accepts
  that as "does not count".
- **Binding.** A credential id is unique across Forge and belongs to one
  user. Step-up accepts only the signed-in user's own passkeys. Sign-in
  refuses disabled accounts.
- **Parsing.** The CBOR reader accepts only what WebAuthn uses: integers,
  byte and text strings, arrays, maps, `true`, `false` and `null`, with
  definite lengths. It refuses tags, floats, indefinite lengths, duplicate
  map keys and trailing bytes, and limits strings to 16 KiB, arrays and maps
  to 64 entries and nesting to 8 levels. Malformed input is an error, never
  a crash; randomised tests feed it tens of thousands of malformed inputs.

### What Forge stores

Only public data: the credential id, the public key with its algorithm, the
signature counter, a name and two dates. The private key never leaves the
authenticator. A copy of the database lets nobody sign in with a passkey.
Outstanding challenges are stored as hashes and expire after 5 minutes.

### Limitations

- Passkeys need the console on HTTPS or `localhost`; browsers refuse
  WebAuthn elsewhere.
- A passkey is bound to the host name it was registered on, as browsers
  scope them. On another name or IP of the same Forge it is not offered;
  sign in with a password and code there, or register another passkey.
- No attestation means no allow-list of authenticator models.
- Recovery is by an administrator or on the server host; see
  [Passkeys](/docs/guides/passkeys/#recover-access).

## Request addresses

Forge has no fixed address. Each request carries the address the client used:
the `Host` header, or the last `X-Forwarded-Host` and `X-Forwarded-Proto` from
a peer in `[security] trusted_proxies`. Forwarded headers from any other peer
are ignored. A host that is not a valid name or IP literal with an optional
port gets `400`.

- **Allowed hosts.** The `server.allowed_hosts` setting (default `*`) lists
  the names Forge answers on. Any other host gets `421 Misdirected Request`
  with no body. `*.example.com` matches only names under that domain, at a
  dot boundary. Narrow the list once Forge has a domain.
- **Same-origin checks.** A browser `Origin` on WebSockets (events,
  terminal, tunnels) and `/mcp` must equal the request's own origin on an
  allowed host, or the pinned address. Requests without `Origin` (the CLI,
  scripts) pass. State-changing API calls also need the double-submit CSRF
  token.
- **Links.** Request-derived addresses go only into links a signed-in user
  receives (Add node commands, device and approval links, the OpenAPI
  `servers` entry) and into the token-protected `/install.sh` and
  `/install.ps1`. Emails, alerts, webhooks, OIDC and OAuth always use the
  pinned public address, or the address an administrator last signed in on.
  A forged `Host` header therefore cannot redirect links that leave Forge.
- **HSTS** is sent only on HTTPS responses, never to a plain-HTTP name.

## Pinned server key

Agents do not check the control plane's host name. They check its key.

The agent listener (`3843`) and the analytics upload listener (`3832`)
present certificates from Forge's internal CA for one long-lived
control-plane key. Each agent keeps a pin set: the SHA-256 fingerprints of
the SubjectPublicKeyInfo of the keys it trusts. It accepts the server only if
the leaf certificate's key is pinned, the certificate is within its validity
period, and it allows server authentication. Another certificate from the
same CA with a different key is refused. Because names are not checked, a
node reaches Forge on any IP or name, through NAT, or across networks.

```
  enrollment token ── server_key_sha256 ──► pin set (pins.toml, root-owned)
                                               ▲
  heartbeat reply ── current + next keys, ─────┘ applied by the root helper
                     signed by the command key   after it checks the signature
```

- **Enrollment** pins the key named in the enrollment token.
- **Renewal** issues a new certificate for the same key, swapped into the
  listener without a restart. Agents see no change.
- **Rotation.** The server holds a `current` and a `next` key. Every
  heartbeat reply announces both, signed with the command key, and agents add
  the next key to their pin set and report what they pin. After
  `[agent_tls] rotate_days` (default 365), once the next key has been
  announced for `grace_days` (default 30) and every online agent has it,
  the listeners switch. The old key is kept sealed for one more rotation.
  Agents drop a key 30 days after Forge stops announcing it.
- **Manual control.** `forge-server agent-key show|list|rotate [--now]` on the
  server, `forge-agent pins list|add|remove|fetch` on a node. A node offline
  for the whole grace period, or a forced switch, needs `pins add`.
- **At rest.** Private keys are sealed with the server's envelope key in the
  database. Only fingerprints appear in output, the API and audit. On Linux
  nodes the pin set is owned by root; the agent user cannot change it, and
  the root helper writes only announcements signed by the command key over
  an already pinned link. A set holds at most 8 keys.
- **Older agents.** An agent with no pins keeps checking the CA and the host
  name until it receives a pin over its existing link.

The trade-off: trust on first use moves to enrollment. The enrollment token
carries the fingerprint, so a token from the console is enough. `pins fetch`
trusts whatever answers, and asks you to compare fingerprints first.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Agent log or `forge-agent status`: `server key sha256:… is not pinned (pinned: sha256:…). If the server's key changed, run 'forge-server agent-key show' on the server and 'forge-agent pins add <fingerprint>' on this node. If it did not, something else is answering on <host>:3843.` | The server switched keys while this node missed the announcement, or another machine answers on that address | Compare with `forge-server agent-key show`. If it matches, `forge-agent pins add <fingerprint>`; if not, fix the address. See [The key mismatch error](/docs/reference/cli/forge-agent/#the-key-mismatch-error) |
| Browser or API: `421 Misdirected Request` | The host is not in **Allowed hosts** | Add it in **Settings → Server**, or run `forge-server allowed-hosts clear` on the server host and restart |
| Terminal or live updates close at once on a new name | The `Origin` is not the request's own origin on an allowed host, often a proxy that is not in `trusted_proxies` and rewrites `Host` | Add the proxy to `trusted_proxies` and send `X-Forwarded-Host` (see [Reverse proxy headers](/docs/reference/configuration/#reverse-proxy-headers)) |
| No passkey offered on a new name or IP | Passkeys belong to the name they were registered on; browsers offer them only on HTTPS or `localhost` | Sign in with password and code, then register a passkey on this name |

## Secrets

Envelope encryption: each secret gets its own AES-256-GCM data key, wrapped by
a key-encryption key from a `KeyProvider` (a local key file today; TPM, KMS and
OpenBao later). Additional data binds each ciphertext to its record.

## Port tunnels

A port tunnel is the only remote path into a node besides the browser
terminal, and it uses the same API port as everything else: nothing listens
on a node for it. A tunnel points at a service, not a bare port; Forge
resolves it in its service catalogue, checks an environment role
(`network.tunnel`) or an explicit grant per (node, port), and the agent
dials only local addresses. Every 15 seconds the server re-checks the
sign-in, the role or grant and the expiry; a failed check closes the tunnel
within 30 seconds. See [Port tunnels](/docs/concepts/port-tunnels/).

:::note
A tunnel reaches services that trust their own machine (local `trust`
authentication, unauthenticated admin UIs). Roles on an environment and grants
decide who gets that far; prefer short grants.
:::

## Containment

| Compromised | Reaches | Stops spreading |
| --- | --- | --- |
| User account | That user's scopes | MFA guardrail, audit, revocation |
| Plugin | Its approved host functions | Intersection, sandbox, quarantine |
| Agent / server | That server | Per-agent cert, signed commands, no fleet credentials on nodes |
| Machine on the agent's network path | Nothing | Agents accept only a pinned control-plane key |
| Control plane | Everything | Admin MFA, signed agent commands, offline backups |

## Hygiene

- One crypto library (`ring`); no OpenSSL.
- `cargo deny` (licenses, advisories, bans) and `cargo audit` in CI.
- Security headers: CSP, `X-Frame-Options: DENY`, `nosniff`, `no-referrer`.
- Request logs record paths only — never query strings, bodies or tokens.
- Rust prevents memory bugs, not authorization mistakes: threat modelling and
  penetration tests remain required.
