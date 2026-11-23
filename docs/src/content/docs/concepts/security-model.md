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
  `public_url` exactly (scheme, host and port), and cross-origin requests
  are refused.
- **Relying party.** The RP ID is the host of `public_url`. The SHA-256 of
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
- Passkeys are bound to the host of `public_url`. Changing that host makes
  every registered passkey unusable.
- No attestation means no allow-list of authenticator models.
- Recovery is by an administrator or on the server host; see
  [Passkeys](/docs/guides/passkeys/#recover-access).

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
| Control plane | Everything | Admin MFA, signed agent commands, offline backups |

## Hygiene

- One crypto library (`ring`); no OpenSSL.
- `cargo deny` (licenses, advisories, bans) and `cargo audit` in CI.
- Security headers: CSP, `X-Frame-Options: DENY`, `nosniff`, `no-referrer`.
- Request logs record paths only — never query strings, bodies or tokens.
- Rust prevents memory bugs, not authorization mistakes: threat modelling and
  penetration tests remain required.
