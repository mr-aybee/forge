---
title: REST API
description: Every /api/v1 endpoint with its required permission.
sidebar:
  order: 2
---

Base path `/api/v1`. JSON in and out. Errors use RFC 9457 problem details:

```json
{"type": "https://docs.orbyn.dev/errors/forbidden", "title": "forbidden", "status": 403, "detail": "server.manage on server:…"}
```

A deployment is one workspace (D-016), so routes act on it without a
workspace id. If the database ever holds more than one, those routes return
`503` `multiple_workspaces` rather than guess which one is meant, and the
server logs an error at start.

**Machine-readable description:** `GET /api/openapi.json` (public) is an
OpenAPI 3.1 document generated from the server's own routes. Request bodies,
query parameters and many answers carry JSON schemas under
`components/schemas`; ids are strings and times are epoch milliseconds.
`x-forge-schema` says whether an operation is `typed`, `partial` or
`untyped`. One body is documented here only:
`PUT /clusters/{id}/profile`. `x-forge-action` names the Cedar action and
`x-forge-mfa` the step-up requirement. `forge api describe <id>` prints one
operation with its schemas.

**Authentication:** `forge_session` cookie + `X-CSRF-Token` on non-GET
requests, or `Authorization: Bearer <token>`.
Tokens issued to an MCP connector are refused here with `401`
(`invalid_token`, `this token is for the MCP endpoint only`); see
[MCP connectors](#mcp-connectors).

## Idempotency-Key

Send `Idempotency-Key: <key>` with a `POST`, `PUT`, `PATCH` or `DELETE` to
make a retry safe: Forge runs the request at most once per key and person
for 24 hours. The `forge` CLI sends one with every change.

| Rule | Value |
| --- | --- |
| Key | 1 to 255 visible ASCII characters, such as a random UUID. Anything else is `400` |
| Scope | The authenticated person and the kind of credential: an MFA-verified session, a session without MFA, or an API key. Another person, or a weaker credential of the same person, with the same key runs on its own and never sees your response |
| Same key, same request | The stored response again: same status and body, with `Idempotent-Replayed: true`. Nothing runs again; the replay is audited as `idempotency.replay` (path only) |
| Same key, still running | `409` `idempotency_in_progress` with `Retry-After: 1` |
| Same key, different request | `422` `idempotency_key_reused`. The request is compared by method, path with query, and body |
| What is stored | Only `2xx` answers with a JSON or empty body up to 256 KiB, encrypted with the server's vault key, because some carry a one-time secret. Answers on sign-in, token, reveal and other secret-bearing paths (`/auth/`, `/oauth/`, `/me/`, `/secrets`, `/vault/`, `reveal`, `token`, `/sftp`, `grant`, `/enroll`, `/mcp`) are never stored: the header is ignored there |
| Limit | 1,000 unexpired keys per person and credential kind; beyond that, requests run without a stored answer |
| Failures | Any other answer (`4xx`, `5xx`, a step-up demand) frees the key, so a retry runs again |
| Lifetime | 24 hours. A key whose request was cut off by a server restart is free again after 10 minutes |

Requests without the header behave as before. `GET` ignores it.

## Discovery (public)

| Method | Path | Notes |
| --- | --- | --- |
| GET | `/api/version` | Product, version, API versions, plugin API, agent protocol |
| GET | `/api/capabilities` | Feature flags, including declared-but-disabled extension points |
| GET | `/api/health` | `ok` when the database answers |

## Session

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/auth/captcha[?login=<name>]` | public — whether the next password sign-in needs the [image CAPTCHA](#image-captcha), and a new picture when it does. `429` `throttled` after 60 pictures for one address (IPv6: one /64) in 15 minutes |
| GET | `/auth/challenge[?login=<name>]` | public — a single-use [login challenge](#login-challenge) for the next password sign-in, or `{required: false}` when `[security] login_challenge` is off. `429` `throttled` when too many challenges are outstanding |
| POST | `/auth/login` | public (throttled per account and client address, see below) — `{login, password, totp?, challenge_id?, nonce?, captcha_id?, captcha_answer?}`; each authenticator code works once; `login` is a username or an email (`email` is accepted as an alias). `challenge_id` and `nonce` are required while the login challenge is on; `captcha_id` and `captcha_answer` after failed sign-ins, see [Image CAPTCHA](#image-captcha) |
| POST | `/auth/logout` | authenticated |
| GET | `/me` | authenticated |
| GET | `/me/capabilities` | authenticated — which console areas you may open, as flat booleans (see [Console capabilities](#console-capabilities)). Navigation hints only: every endpoint still authorizes its own requests |
| PATCH | `/me` | authenticated — `{username?, email?, display_name, current_password?}`: your own sign-in names; one of username/email must remain. Changing the username or email needs proof of identity like a factor change (see the note under the passkey table): not with an API key, an MFA-verified session if you have a factor, else `current_password`. The display name alone needs none. Audited (`user.profile.update`, also when denied) |
| POST | `/me/password` | web session (throttled) — `{current_password, new_password}`. Ends every other sign-in of the user (web sessions, app and connector tokens; API keys stay) and sets new session and CSRF cookies for this browser. Audited (`user.password.change`) |
| POST | `/auth/step-up` | signed in, not with an API key (`403`); throttled — `{code}`: verifies an authenticator code and replaces the session with an MFA-verified one. A browser session gets new cookies and `{mfa: true, csrf_token}`. A bearer session (apps, the CLI) gets a new token pair, see [Step-up for bearer sessions](#step-up-for-bearer-sessions). `400` without an authenticator app; `401` `invalid_mfa` for a wrong code. Audited (`auth.step_up`, also when denied). Guarded actions return `403` with `title: step_up_required` when only this is missing |
| POST | `/auth/totp` | authenticated — begin TOTP enrollment |
| POST | `/auth/totp/confirm` | `{secret, code, password?}`: stores (or replaces) your authenticator app. Needs proof of identity, see the note below this table. `401` `invalid_mfa` for a wrong code. Audited (`auth.totp.enroll`, also when denied) |
| GET | `/auth/oidc/login`, `/auth/oidc/callback` | public, when SSO is configured. `login` sets the short-lived `forge_oidc` cookie (`HttpOnly`, `SameSite=Lax`, 10 minutes); `callback` returns `400` unless that cookie matches `state` |
| GET | `/oauth/authorize` | web session (redirects to `/login` otherwise). Registered MCP connectors go to the consent page; see [MCP connectors](#mcp-connectors) |
| POST | `/oauth/token` | public — PKCE code, refresh token or [device code](#device-sign-in); optional `resource` (RFC 8707) for connectors only |
| POST | `/oauth/device_authorization` | public — starts a [device sign-in](#device-sign-in) |
| GET, POST | `/api/v1/oauth/device/{code}` | browser session — look up and approve a [device sign-in](#device-sign-in) |

The `/oauth/*` paths sit at the root of the host, outside `/api/v1`.

### Console capabilities

`GET /api/v1/me/capabilities` returns one boolean per gated area of the web
console. The console hides an entry, and shows "You don't have access to
this page" on its route, when the flag is `false`. The flags are hints: the
endpoints behind each page still authorize every request.

Flags marked **anywhere** are `true` when one of your role bindings grants
the action at any scope, so a role on a single cluster or environment is
enough; the lists on those pages show only what you can see. The other flags
ask the authorizer about the workspace or the platform, the resource the
page's main endpoint checks. Every flag is computed as if the session were
MFA-verified: a session without MFA still sees the entry and is asked to step
up when it acts.

| Field | Type | Cedar action and resource | Console areas |
| --- | --- | --- | --- |
| `fleet_view` | boolean | `view`, anywhere | **Clusters**, **Nodes**, **Projects**, **Services**, **Stacks**, **Installed**, **DNS**, **Ingress**, **Object Storage**, **Jobs** |
| `terminal` | boolean | `terminal.open`, anywhere | **Terminal** |
| `package_install` | boolean | `package.manage`, anywhere | **App Store** |
| `software_updates` | boolean | `server.manage`, anywhere | **Updates** |
| `sftp_access` | boolean | `file.read`, anywhere | **SFTP access** |
| `tunnels` | boolean | `network.tunnel` anywhere, or `access.manage` on the workspace | **Tunnels** |
| `alerts_view` | boolean | `alert.view` on the workspace | **Alerts** |
| `alerts_manage` | boolean | `alert.manage` on the workspace | (actions on **Alerts**) |
| `notifications_manage` | boolean | `notification.manage` on the workspace | (notification channels) |
| `audit_view` | boolean | `audit.read` on the platform | **Audit log** |
| `users_manage` | boolean | `user.manage` on the workspace | **Members & roles** |
| `access_manage` | boolean | `access.manage` on the workspace | (access profiles, service accounts) |
| `vault` | boolean | a `platform_admin` role binding | **Credential vault** |
| `forge_updates` | boolean | `system.update` on the platform | **Forge updates** |
| `analytics_configure` | boolean | `analytics.configure` on the platform | (changes on **Settings → Analytics**) |
| `plugins` | boolean | `view` on the platform | **Plugins** |
| `platform_settings` | boolean | `view` on the platform | **Settings** |

**Overview**, **Approvals** and **Profile** have no flag: every signed-in
user sees them.

```json
{"fleet_view": true, "terminal": true, "package_install": true, "software_updates": true,
 "sftp_access": true, "tunnels": true, "alerts_view": false, "alerts_manage": false,
 "notifications_manage": false, "audit_view": false, "users_manage": false,
 "access_manage": false, "vault": false, "forge_updates": false, "analytics_configure": false, "plugins": false,
 "platform_settings": false}
```

This example is an `operator` on one cluster.

### Step-up for bearer sessions

Every `403` with `title: step_up_required` also carries this header
(RFC 9470), so a client can tell it apart from a plain denial. The status
stays `403`:

```
WWW-Authenticate: Bearer error="insufficient_user_authentication", error_description="multi-factor authentication required", acr_values="mfa"
```

To step up, the client sends `POST /api/v1/auth/step-up` with `{code}` and
its bearer token, then retries the request with the new access token. The
response (`Cache-Control: no-store`):

```json
{"access_token": "<access-token>", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "<refresh-token>"}
```

The old access and refresh tokens stop working at once. The new refresh
token's lifetime still counts from the original sign-in, so a step-up does
not extend the session. Audited as `auth.step_up` with `{session: "bearer"}`.
`expires_in` is the access token lifetime in seconds.

### Device sign-in

Device sign-in (RFC 8628) lets the CLI sign in on a machine without a
browser (`forge auth login --device`). A person signed in to the console
approves the code at `<public_url>/device`. See
[Sign in to the CLI on a server or over SSH](/docs/guides/cli-headless-login/).
Requests live in memory for 10 minutes; up to 1,000 can wait at once (at
most 5 from one client address, loopback excepted), and a restart of the
server drops them.

**`POST /oauth/device_authorization`**, form-encoded, public:

| Field | Notes |
| --- | --- |
| `client_id` | Required. Only Forge's own apps: `forge-app` |

Response (`Cache-Control: no-store`):

| Field | Type | Notes |
| --- | --- | --- |
| `device_code` | string | Secret; the client polls with it. Stored only as a hash |
| `user_code` | string | 8 letters as `XXXX-XXXX`, from `BCDFGHJKMNPQRSTVWXZ` |
| `verification_uri` | string | `<public_url>/device` |
| `verification_uri_complete` | string | `<public_url>/device?code=<user_code>` |
| `expires_in` | integer | `600` seconds |
| `interval` | integer | `5`: seconds to wait between polls |

**Poll** `POST /oauth/token`, form-encoded, with
`grant_type=urn:ietf:params:oauth:grant-type:device_code`, `device_code` and
`client_id`. After approval, the answer is a token pair as above. The tokens
are never MFA-verified, whatever the approving session had; the client steps
up for MFA-protected actions. Issuing them is audited as `oauth.device.token`.
The request is single use: once approved or denied, the next poll gets
`expired_token`.

```sh
curl -s https://<forge-host>/oauth/token \
  -d grant_type=urn:ietf:params:oauth:grant-type:device_code \
  -d client_id=forge-app \
  --data-urlencode "device_code=$FORGE_DEVICE_CODE"
```

Errors of both endpoints are RFC 6749 JSON, not problem details:
`400 {"error": "...", "error_description": "..."}`.

| `error` | Meaning |
| --- | --- |
| `invalid_client` | `client_id` is not one of Forge's own apps (`device_authorization`) |
| `invalid_request` | `device_code` or `client_id` is missing |
| `authorization_pending` | Nobody has answered yet; poll again after `interval` |
| `slow_down` | Polled sooner than `interval` after the last poll: add 5 seconds to the interval. From `device_authorization`: 1,000 requests are waiting |
| `access_denied` | The person selected **Deny** |
| `expired_token` | The code expired, is unknown, or was already used |
| `invalid_grant` | `client_id` differs from the one that started the request |

**Approve** in the console. Both endpoints need a browser session (cookie,
and `X-CSRF-Token` on `POST`). A bearer token or API key gets `403`
(`approve device sign-ins in the Forge console`). `{code}` is the user code;
case, spaces and the dash are ignored.

| Method | Path | Notes |
| --- | --- | --- |
| GET | `/api/v1/oauth/device/{code}` | What the approval page shows: `{user_code, client_id, client_name, requested_seconds_ago, expires_in, user, mfa, requested_from, user_agent}`. `user` is your username, else email; `mfa` is always `false` (device tokens are never MFA-verified); `requested_from` is the client address that started the sign-in (behind `[security] trusted_proxies`, from `X-Forwarded-For`) and `user_agent` its client, up to 120 characters. Wrong or expired codes are audited as `oauth.device.lookup` |
| POST | `/api/v1/oauth/device/{code}` | `{approve}` (boolean; unknown fields are refused) → `{approved}`. Audited as `oauth.device.approve` with `client_id` and `user_code`; a denial is recorded with outcome `denied` |

An unknown, expired or answered code gives `404`. Both endpoints count
unknown codes per person: after 10 in 15 minutes they return `429`
`throttled`.

### Login challenge

Password sign-in needs a solved proof of work while `[security]
login_challenge` is on (the default). The capability `auth.login_challenge`
in `GET /api/capabilities` tells you whether it is. Passkey and SSO sign-in
do not use it.

1. `GET /api/v1/auth/challenge?login=<name>`. Pass the sign-in name: a name
   with recent failed sign-ins gets a harder challenge, and a challenge
   fetched without it may be refused as too easy.
2. Find a nonce: 1 to 32 ASCII letters or digits, usually a decimal counter,
   such that SHA-256(salt bytes ‖ nonce bytes) starts with `difficulty` zero
   bits. Decode `salt` from hex first.
3. `POST /api/v1/auth/login` with `challenge_id` (the `id`) and `nonce`.

Response of `GET /auth/challenge`:

| Field | Type | Description |
| --- | --- | --- |
| `required` | boolean | `false` when the server does not ask for a challenge; no other fields then |
| `id` | string | Challenge id, sent back as `challenge_id` |
| `algorithm` | string | Always `"sha256-leading-zero-bits"` |
| `salt` | string | 16 random bytes, hex-encoded |
| `difficulty` | integer | Leading zero bits required. `17` normally; one more per recent failed sign-in, at most `24`. Failures count per sign-in name and per client address, and the higher count applies. A count clears 15 minutes after its last failure. A successful sign-in clears the name's count, not the address's. Loopback addresses are not counted; behind a proxy, the client address comes from `X-Forwarded-For` only with `[security] trusted_proxies` |
| `expires_at` | integer | Epoch ms; the challenge is valid for 2 minutes |

```json
{"required": true, "id": "<challenge-id>", "algorithm": "sha256-leading-zero-bits", "salt": "9f2c…", "difficulty": 17, "expires_at": 1700000120000}
```

A challenge is single use. It is spent by any login attempt that carries it,
also when the nonce or the password is wrong, so fetch a new one for every
attempt, including the second request with `totp` after `mfa_required`. The
throttle is checked first: 10 failed passwords for a name in 15 minutes
return `429` `throttled` whatever the challenge.

| Status | `title` | Meaning |
| --- | --- | --- |
| `400` | `challenge_required` | `challenge_id` or `nonce` is missing |
| `400` | `challenge_failed` | The nonce does not solve it; the challenge is unknown, used or expired; or it is easier than the sign-in name now needs. Fetch a new one with `?login=` |
| `429` | `throttled` | Too many failed sign-ins in 15 minutes: 10 for this name from this client address, 50 from this address (not counted for a loopback peer) or 100 for this name overall; or too many outstanding challenges. The client address comes from `X-Forwarded-For` only behind `[security] trusted_proxies` |

Scripts and API clients solve it the same way. This Python example uses only
the standard library and reads the credentials from the environment:

```python
import hashlib, http.cookiejar, json, os, urllib.parse, urllib.request

BASE = "https://forge.example.com/api/v1"
LOGIN = os.environ["FORGE_LOGIN"]
PASSWORD = os.environ["FORGE_PASSWORD"]
opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

def call(path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, headers={"Content-Type": "application/json"})
    with opener.open(req) as r:
        return json.load(r)

def solve(salt_hex, bits):
    salt = bytes.fromhex(salt_hex)
    nonce = 0
    while int.from_bytes(hashlib.sha256(salt + str(nonce).encode()).digest(), "big") >> (256 - bits):
        nonce += 1
    return str(nonce)

c = call("/auth/challenge?login=" + urllib.parse.quote(LOGIN))
body = {"login": LOGIN, "password": PASSWORD}
if c["required"]:
    body |= {"challenge_id": c["id"], "nonce": solve(c["salt"], c["difficulty"])}
session = call("/auth/login", body)
print("signed in; MFA-verified:", session["mfa"])
```

:::tip
For automation, an API key (`Authorization: Bearer <token>`) needs no login
challenge.
:::


### Image CAPTCHA

After `[security] captcha_after_failures` failed password sign-ins (default
`2`) for a sign-in name, or from the client address, within 15 minutes, the
next password sign-in also needs the characters from a picture. Passkey and
SSO sign-in never need it.

1. After a failed sign-in, `GET /api/v1/auth/captcha?login=<name>`.
2. If `required` is `true`, show `image` and ask for the characters.
3. Send `captcha_id` (the `id`) and `captcha_answer` with the next
   `POST /api/v1/auth/login`, together with a solved login challenge.

| Field | Type | Description |
| --- | --- | --- |
| `required` | boolean | `false` when the next sign-in from this address as this name needs no picture; no other fields then |
| `id` | string | Picture id, sent back as `captcha_id` |
| `image` | string | A `data:image/png;base64,…` URL: 220 × 80 pixels, grayscale |
| `length` | integer | Characters in the picture: `6` |
| `expires_at` | integer | Epoch ms; the picture is valid for 2 minutes |

The characters are uppercase letters and digits without look-alikes
(`ACDEFGHJKMNPQRTUVWXY34679`). The answer is not case-sensitive and spaces
are ignored. A picture works once, only from the address it was issued to
(IPv6 addresses count per /64 network, for binding, failure counts and the
per-address picture limit).
Any login attempt that carries it uses it up, also with a wrong answer, so
fetch a new one after every failure.

| Status | `title` | Meaning |
| --- | --- | --- |
| `400` | `captcha_required` | This sign-in needs a picture and `captcha_id` or `captcha_answer` is missing |
| `400` | `captcha_failed` | Wrong characters, or the picture is unknown, used, expired or from another address. Fetch a new one |
| `429` | `throttled` | Over 60 pictures for this address (IPv6: this /64) in 15 minutes |

When more than 10,000 pictures are outstanding, the oldest are dropped, so a
flood of requests never stops other clients from getting one. Pictures live
in the memory of one server process: behind a load balancer with several
instances, use client-address affinity.

:::note
There is no audio alternative yet. Passkey sign-in never shows the picture
and is the accessible way in; SSO sign-in does not show it either.
:::

## Pins

Your pinned projects, services, nodes and clusters, which the console shows
in the **Pinned** section of the sidebar. Pins are personal: every request
reads or writes only the caller's own pins, and no role can read another
user's. The CLI reaches these through `forge api call`.

| Method | Path | Notes |
| --- | --- | --- |
| GET | `/me/pins` | authenticated — `{items: [Pin]}` ordered by `position`. Pins whose entity no longer exists, or that you can no longer `view`, are left out but stay stored, so they come back if access returns |
| PUT | `/me/pins/{kind}/{id}` | authenticated — pins the entity at the end. Idempotent: pinning again changes nothing. `204`. `404` if the entity does not exist or you cannot `view` it (also audited as a denial); `400` for an unknown `kind` or a malformed `id`; `409` `pin_limit` when you hold 50 pins already. Audited (`user.pin.add`) |
| DELETE | `/me/pins/{kind}/{id}` | authenticated — unpins. Idempotent, `204` even when it was not pinned. Audited (`user.pin.remove`) |
| PUT | `/me/pins/order` | authenticated — `{items: [{kind, id}]}`: these pins first, in this order; pins you leave out follow in their current order. Unknown entries are ignored. `204`. Audited (`user.pin.reorder`) |

`kind` is one of `project`, `service`, `node` or `cluster`; `id` is the
entity's id. A pin in the list:

| Field | Type | Description |
| --- | --- | --- |
| `kind` | string | `project`, `service`, `node` or `cluster` |
| `id` | string | The entity's id |
| `label` | string | The entity's current name (a node's console name, else its hostname) |
| `sublabel` | string, optional | Context: the environment of a service, the cluster of a node. Absent for projects, clusters and unassigned nodes |
| `position` | integer | Sort key, ascending |
| `created_at` | integer | When it was pinned, epoch milliseconds |

## Passkeys

WebAuthn passkeys for sign-in and step-up. See [Passkeys](/docs/guides/passkeys/)
for the tasks and [Security model](/docs/concepts/security-model/#how-forge-verifies-passkeys)
for the checks.

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/me/passkeys` | authenticated — your passkeys |
| POST | `/me/passkeys/register/begin` | `{password?}` — creation options. Needs proof of identity, see the note below this table |
| POST | `/me/passkeys/register/finish` | authenticated — verifies and stores the passkey. Audited (`auth.passkey.add`, also when refused) |
| PATCH | `/me/passkeys/{id}` | authenticated — `{name}`; your own passkeys only |
| DELETE | `/me/passkeys/{id}` | your own passkeys only. Needs a session that verified a factor (see the note below this table). Audited (`auth.passkey.remove`, also when denied) |
| POST | `/auth/passkey/begin` | public — request options for a passwordless sign-in |
| POST | `/auth/passkey/finish` | public (throttled) — signs in; the session is MFA-verified. Audited (`auth.login`, `method: passkey`) |
| POST | `/auth/step-up/passkey/begin` | signed in, not with an API key (`403`) — request options limited to your passkeys |
| POST | `/auth/step-up/passkey/finish` | signed in, not with an API key (`403`); throttled — replaces the session with an MFA-verified one. Audited (`auth.step_up`, `method: passkey`) |
| DELETE | `/users/{id}/passkeys` | `user.manage` on the platform (Platform admin) **and** an MFA-verified session — recovery. Audited (`auth.passkey.remove_all`) |

:::note[Proof of identity for sign-in factors]
Changing your own sign-in factors (`/auth/totp/confirm`, `/me/passkeys/register/begin`, `DELETE /me/passkeys/{id}`) needs proof of identity, not only a session: API keys get `403`. If the account has an authenticator app or a passkey, the session must have verified one itself (sign-in with a code or passkey, or `/auth/step-up`), else `403` `step_up_required`. This applies even when MFA is turned off for the server, where every session otherwise counts as MFA-verified. With no factor yet, the body carries `password` (your current password; `401` `password_required`, throttled, `429` audited). A single sign-on account without a password instead needs a browser sign-in from the last 5 minutes (`403` `reauthentication_required`). Denials are audited.
:::

The browser must reach Forge at `public_url`: the relying party ID is its
host and every response must come from its origin. Binary values in
requests and responses are base64url without padding. The `*/begin` bodies
are empty.

**Passkey object** (list, register):

| Field | Type | Notes |
| --- | --- | --- |
| `id` | string | Forge's id for the passkey, used in `PATCH` and `DELETE` |
| `name` | string | 1–60 characters, no control characters |
| `created_at` | integer | Epoch ms |
| `last_used_at` | integer \| null | Epoch ms of the last sign-in or step-up |

Responses never contain the credential id or public key.

**`register/begin` response:** `PublicKeyCredentialCreationOptions` for
`navigator.credentials.create()`: `challenge`, `rp` (`id`, `name: "Orbyn Forge"`),
`user` (`id` = base64url of the user id, `name` = username, else email, else
id, `displayName`), `pubKeyCredParams` (algorithms `-7`, `-8`, `-257`),
`timeout: 300000`, `attestation: "none"`, `authenticatorSelection`
(`residentKey: "required"`, `requireResidentKey: true`,
`userVerification: "required"`) and `excludeCredentials` (your existing
passkeys). Refused with `400` when you already have 20 passkeys.

**`register/finish` request:**

| Field | Type | Notes |
| --- | --- | --- |
| `client_data_json` | string | base64url `response.clientDataJSON` |
| `attestation_object` | string | base64url `response.attestationObject` |
| `name` | string, optional | 1–60 characters; empty or missing becomes `Passkey` |

Returns `201` with the passkey object.

**`auth/passkey/begin` and `auth/step-up/passkey/begin` response:**
`PublicKeyCredentialRequestOptions` for `navigator.credentials.get()`:
`challenge`, `rpId`, `timeout: 300000`, `userVerification: "required"` and
`allowCredentials` (empty for sign-in, so the browser offers discoverable
passkeys; your passkeys for step-up). Step-up begin returns `400` when you
have no passkey.

**`auth/passkey/finish` and `auth/step-up/passkey/finish` request:**

| Field | Type | Notes |
| --- | --- | --- |
| `credential_id` | string | base64url `rawId` |
| `client_data_json` | string | base64url `response.clientDataJSON` |
| `authenticator_data` | string | base64url `response.authenticatorData` |
| `signature` | string | base64url `response.signature` |

Each field is at most 64 KiB. Sign-in returns
`{"user": {…}, "mfa": true, "csrf_token": "…"}` and sets the `forge_session`
and `forge_csrf` cookies. Step-up returns `{"mfa": true, "csrf_token": "…"}`,
sets new cookies and ends the old session. For step-up, the passkey must be
one of yours.

**`DELETE /users/{id}/passkeys` response:** `{"removed": <count>}`. The
user's authenticator app is not touched.

**Errors:**

| Status | `title` | When |
| --- | --- | --- |
| `400` | `bad_request` | Invalid base64url or a field over 64 KiB; invalid name; passkey already registered; 20 passkeys reached; step-up begin without a passkey |
| `401` | `invalid_passkey` | Verification failed: challenge expired (5 minutes) or already used; unknown passkey; wrong origin, RP ID or client data type; user presence or verification missing; unsupported attestation format or algorithm; malformed data; bad signature; signature counter went backwards |
| `403` | `forbidden` | Account disabled (sign-in); an API key on `register/begin` or step-up; `user.manage` missing |
| `403` | `step_up_required` | `DELETE /users/{id}/passkeys` without an MFA-verified session |
| `404` | `not_found` | `PATCH` or `DELETE` of a passkey that is not yours or does not exist |
| `429` | `throttled` | 10 failures in 15 minutes: per credential id for sign-in, per account for step-up (shared with code step-up) |

## MCP connectors

OAuth discovery, registration, consent and Connected apps for AI agents
that use the [MCP endpoint](/docs/reference/mcp/). The discovery and
registration paths sit at the root of the host, outside `/api/v1`. While
`[mcp] enabled` is `false`, the protected resource metadata and
`/oauth/register` answer `404` and no new consent can start; listing and
revoking connections and clients still work. Without a pinned public
address (runtime setting or `public_url`), discovery, registration,
consent and `/mcp` answer `409` with code `no_public_address`. See
[MCP connectors](/docs/concepts/mcp-connectors/) for the flow.

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/.well-known/oauth-authorization-server` | public — RFC 8414 metadata; `registration_endpoint` only when MCP is enabled |
| GET | `/.well-known/oauth-protected-resource`, `/.well-known/oauth-protected-resource/mcp` | public — RFC 9728 metadata for `<public_url>/mcp` |
| POST | `/oauth/register` | public, capped by `[mcp] registrations_per_hour` — RFC 7591 registration of a public client. Audited (`oauth.client.register`, actor `connector:anonymous`) |
| GET | `/api/v1/oauth/consent/{id}` | web session of the person who started the sign-in — what the consent page shows |
| POST | `/api/v1/oauth/consent/{id}` | web session of the person who started the sign-in — `{approve, mode?}`. Audited (`connector.approve`, also when denied) |
| GET | `/api/v1/connected-apps` | authenticated — your connections; `?all=true` lists everyone's and needs `user.manage` on the workspace |
| POST | `/api/v1/connected-apps/{id}/revoke` | authenticated — your own connection, or anyone's with `user.manage`. Cancels the connection's pending approvals. Audited (`connector.revoke`, and `mcp.approval.cancel` per cancelled approval) |
| GET | `/api/v1/oauth/clients` | `user.manage` on the workspace — registered connectors |
| POST | `/api/v1/oauth/clients/{id}/revoke` | `user.manage` on the workspace — blocks the connector, ends every connection made with it and cancels their pending approvals. Audited (`oauth.client.revoke`, and `mcp.approval.cancel` per cancelled approval) |

Metadata fields, registration fields and errors, and the redirect URI rules
are in the [MCP reference](/docs/reference/mcp/#oauth-endpoints).

**Consent.** `/oauth/authorize` sends the browser to `/connect?request=<id>`.
The request lives in memory for 10 minutes; up to 1,000 can wait at once.
Only a browser session (cookie with CSRF) can read or answer it; a bearer
token or API key gets `403` (`approve connectors in the Forge console`),
and another user gets `404`.

`GET /api/v1/oauth/consent/{id}` response:

| Field | Type | Notes |
| --- | --- | --- |
| `request_id` | string | The `{id}` |
| `client` | object | `client_id`, `name` (chosen by the client), `registered_at` (epoch ms) |
| `redirect_uri` | string | Where the browser goes after the answer |
| `redirect_host` | string | `host[:port]` of `redirect_uri`, or `scheme://host` for an app scheme |
| `resource` | string | `<public_url>/mcp` |
| `modes` | string array | Modes allowed by `[mcp] max_mode`, lowest first: `read_only`, `ask`, `auto` |
| `default_mode` | string | `ask`, or `max_mode` when that is lower |
| `user` | string | The signed-in person's username, else email |

`POST /api/v1/oauth/consent/{id}` body (unknown fields are refused):

| Field | Type | Notes |
| --- | --- | --- |
| `approve` | boolean | Required. `false` denies |
| `mode` | string | `read_only`, `ask` or `auto`; default `ask`, or `max_mode` when that is lower. Above `max_mode` gives `403` |

The response is `{"redirect": "<url>"}`: `redirect_uri` with `code`,
`state` and `iss` after approval, or `error=access_denied`, `state` and
`iss` after a denial. The request is used up either way.

**Connection object** (`GET /api/v1/connected-apps` returns
`{"connections": [...]}`, newest first, revoked ones included):

| Field | Type | Notes |
| --- | --- | --- |
| `id` | string | `ac_…` |
| `user_id` | string | The person who approved it |
| `client_id` | string | `mcp_…` |
| `client_name` | string | Name the client registered |
| `redirect_host` | string | As shown on the consent page |
| `mode` | string | `read_only`, `ask` or `auto` |
| `created_at` | integer | Epoch ms |
| `last_used_at` | integer \| null | Epoch ms; updated at most once a minute |
| `revoked_at` | integer \| null | Epoch ms |

`POST /api/v1/connected-apps/{id}/revoke` returns
`{"revoked": "<id>", "cancelled_approvals": <n>}`. The connection's access
and refresh tokens stop working at once, and its pending approvals move to
`cancelled`.

**Client object** (`GET /api/v1/oauth/clients` returns `{"clients": [...]}`):

| Field | Type | Notes |
| --- | --- | --- |
| `client_id` | string | `mcp_…` |
| `name` | string | Chosen by the client |
| `redirect_uris` | string array | Registered redirect URIs |
| `created_at` | integer | Epoch ms |
| `created_ip` | string \| null | Not recorded yet; always `null` |
| `revoked_at` | integer \| null | Epoch ms |
| `last_used_at` | integer \| null | Epoch ms |

`POST /api/v1/oauth/clients/{id}/revoke` returns
`{"revoked": "<id>", "cancelled_approvals": <n>}`; an unknown or already revoked client gives `404`.

## Approvals

Changes an MCP connector asked for in **Ask** or **Auto** mode wait here
until the person who owns the connection approves or denies them. See
[MCP reference: Approvals](/docs/reference/mcp/#approvals) for when a tool call
is queued, and [MCP connectors](/docs/concepts/mcp-connectors/#how-approvals-work)
for the model.

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/approvals` | authenticated, not a connector — your approvals, newest first, at most 200. `?status=<status>` filters; `?all=true` lists everyone's and needs `user.manage` on the workspace |
| GET | `/approvals/{id}` | the owner; anyone with `user.manage` on the workspace; or, through `forge_approval_status`, the connector connection that asked. Others get `404` |
| POST | `/approvals/{id}/approve` | the owner, from a web session with CSRF; also an MFA-verified session when `mfa_required` is true and the server requires MFA. The connection must still be active, `[mcp] enabled` must still be true, and the connection's mode under the current `[mcp] max_mode` must still allow the tool. Runs the request once as you and returns the approval. Audited (`mcp.approval.decide`); every refusal is audited with outcome denied and the reason |
| POST | `/approvals/{id}/deny` | the owner, from a web session with CSRF, an API key or a CLI bearer token (`forge approvals deny`); never a connector token. Nothing runs; returns the approval. Audited (`mcp.approval.decide`, outcome denied); refusals are audited the same way, with the reason |

Approving replays the stored request through the `/api/v1` router with
your session cookie and CSRF token, never the connector's token. The
handler checks Cedar and the MFA guardrail against you, and its audit
entry has you as actor and `via: {"approval": "<id>", "connector": "<name>"}`.
The response is `200` with the approval in status `executed` or `failed`;
a handler error is recorded in `error`, not returned as the HTTP status.

**Approval object:**

| Field | Type | Notes |
| --- | --- | --- |
| `id` | string | `ap_…` |
| `kind` | string | `mcp.tool` |
| `status` | string | `pending`, `approved` (running now), `executed`, `failed`, `denied`, `expired` or `cancelled` (its connection or client was revoked). A pending approval past `expires_at` reads as `expired`, and is stored as `expired` the next time any connector queues a request |
| `user_id` | string | The person who owns the connection and decides |
| `connection_id` | string \| null | `ac_…`: the connection that asked |
| `requester` | string | The connector's registered name |
| `action` | string | The tool, such as `forge_deploy_stack` |
| `request` | object | What runs: `method`, `uri` (under `/api/v1`), `body` (JSON or `null`) and `arguments` (the tool's arguments as the agent sent them). Data from an AI agent |
| `summary` | string | `<tool title>: <method> <uri>` |
| `mfa_required` | boolean | The tool is destructive, or its route's Cedar action is under the MFA guardrail. Approving then needs an MFA-verified session when the server requires MFA |
| `created_at` | integer | Epoch ms |
| `expires_at` | integer | Epoch ms; 15 minutes after `created_at` |
| `decided_by` | string \| null | User id of the person who decided |
| `decided_at` | integer \| null | Epoch ms |
| `result` | any \| null | The REST response, once `executed` |
| `error` | string \| null | Why it failed, once `failed` |

`GET /approvals` returns `{"approvals": [...]}`.

| Status | Title | Cause |
| --- | --- | --- |
| `403` | `forbidden` | `GET /approvals` with a connector token (`connectors read only their own requests (forge_approval_status)`); `?all=true` without `user.manage`; approve with an API key, a bearer token or a connector token (`approve requests in the Forge console`); deny with a connector token (`connectors cannot decide approvals`); approve or deny someone else's approval (`only the person whose connector asked can approve it` or `… can decide it`) |
| `403` | `forbidden` | Missing or invalid CSRF token |
| `403` | `step_up_required` | The request is MFA-guarded, the server requires MFA and your session is not MFA-verified |
| `404` | `not_found` | Unknown id, or an approval you may not see |
| `409` | `conflict` | `this request was already decided or expired` (also for `cancelled`); approve only: `the connector that asked was disconnected` (connection or client revoked), `AI connectors are turned off on this Forge`, `the connector's mode no longer allows this change` (for example `[mcp] max_mode` lowered to `read_only`) |

Approvals are stored in the `approvals` table (migration `0014`). The table
is generic (`kind`); `mcp.tool` is the only kind today. Decided, expired and
cancelled approvals are deleted 90 days after `created_at`, when the next
request is queued; the audit log keeps the record.

## Registry

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/workspace` | `view` — the deployment's single workspace |
| GET / POST | `/tenants` | `view` / `workspace.manage` on the workspace |
| GET | `/projects` | `view` (filtered per project) — the projects (products) with their environments. See [Projects](#projects) |
| POST | `/projects` | `workspace.manage` on the workspace → `201` |
| GET | `/projects/{id}` | `view` on the project |
| PUT | `/projects/{id}` | `workspace.manage` on the project — rename or change the description |
| DELETE | `/projects/{id}` | `workspace.manage` on the project → `204`; `409` while it still has environments |
| GET | `/environments?cluster_id=<id>&project_id=<id>` | `view` (filtered per environment). See [Environments](#environments) |
| POST | `/environments` | `workspace.manage` on the cluster and on the project → `201`. See [Environments](#environments) |
| PUT | `/environments/{id}` | `workspace.manage` on the environment — rename → `200`; `409` when the cluster has that name |
| PUT | `/environments/{id}/cluster` | `workspace.manage` on the environment and on the target cluster → `200`. See [Environments](#environments) |
| PUT | `/environments/{id}/project` | `workspace.manage` on the environment and on the project — move an environment to another project, or attach one that has none → `200` |
| DELETE | `/environments/{id}` | `workspace.manage` on the environment → `204`; `409` while it still has services or stacks, or owns an object store |
| GET / POST / PUT / DELETE | `/vault/items`, `/vault/items/{id}` | platform administrators only ([Credential vault](/docs/guides/credential-vault/)) |
| POST | `/vault/items/{id}/reveal` | platform administrator + `secret.read` (step-up MFA), browser session |
| POST | `/clusters/{id}/fabric/apply`, `/clusters/{id}/fabric/nodes/{node}/resync` | `workspace.manage` + `server.command` (MFA) on the members |
| GET / PUT / DELETE | `/clusters/{id}/fabric` | `view` / `workspace.manage` on the cluster ([Cluster network](/docs/guides/cluster-network/)) |
| POST | `/clusters/{id}/fabric/suggest`, `/fabric/suggest` | `view`: next free subnet and default domain |
| GET | `/users` | `user.manage` on platform |
| POST | `/users` | `user.manage` on platform **and** an MFA-verified session (`403` `step_up_required`; denials audited as `user.create`) |
| GET | `/role-bindings` | own bindings, or `user.manage` on the scope |
| POST | `/role-bindings` | `user.manage` on the scope **and** an MFA-verified session (`403` `step_up_required`; denials audited as `role.grant`) |
| GET | `/access/catalog` | `view` on the workspace — `{actions, scopes}`. Each action is `{action, area, label, description, level, mfa, roles}`: `area` (string) groups it in the role editor, `label` and `description` are for people, `level` is `read`, `write` or `admin`. `scopes` lists the assignable scope kinds: `workspace`, `cluster`, `project`, `environment`, `server`, `service` |
| GET | `/access/profiles` | `view` on the workspace — every access profile with its current `actions` (built-ins first) |
| POST | `/access/profiles` | `access.manage` (MFA) — `{name, description?, actions}`: a custom profile at revision 1. `409` when the name is taken. Audited (`access.profile.create`) |
| GET | `/access/profiles/{id}` | `view` on the workspace — the profile and `revisions` (`{revision, actions, created_by, created_at}`) |
| PUT | `/access/profiles/{id}` | `access.manage` (MFA) — `{actions, expected_revision}`: publishes the next revision; every assignment moves to it. `?dry_run=true` returns `{added, removed, assignments, users, blocked}` and changes nothing; `users` counts the people reached directly and through groups. `409` for built-in or archived profiles, a stale `expected_revision`, or `escalation` when you lack an added action at an assigned scope (listed in `blocked`). Audited (`access.profile.publish`) |
| DELETE | `/access/profiles/{id}` | `access.manage` (MFA) — archives: its assignments stop granting at once → `{archived, assignments_unlinked}`. `409` for built-ins. Audited (`access.profile.archive`) |
| POST | `/access/profiles/{id}/duplicate` | `access.manage` (MFA) — `{name}`: an editable copy of the current actions. Audited with the source profile and revision |
| GET | `/access/assignments` | assignments at scopes where you hold `user.manage`, your own, and those of groups you are in. Each is `{id, user_id, group_id, profile_id, scope, expires_at, created_by, created_at}`; exactly one of `user_id` and `group_id` is set, the other is `null` |
| POST | `/access/assignments` | `user.manage` on the scope **and** an MFA-verified session (`403` `step_up_required`) — `{user_id or login or group_id, profile_id, scope, expires_in_minutes?}`. `group_id` gives the profile to every member of the group and cannot be combined with `user_id` or `login` (`400`); `404` for an unknown user or group. `scope` is `workspace:`, `cluster:`, `project:`, `environment:`, `server:` or `service:<id>` (`422` otherwise). You must hold every action of the profile over the scope (`403` `escalation`); see [Held over a scope](/docs/concepts/authorization/#held-over-a-scope). `409` when the profile was archived or published meanwhile. Expired assignments stop granting at once and are removed within 30 s. Audited (`access.assign`, with `user` or `group`) |
| DELETE | `/access/assignments/{id}` | `user.manage` on its scope; no MFA. Audited (`access.unassign`) |
| GET | `/access/groups` | `view` on the workspace — every group: `{id, name, description, created_by, created_at, members}`; `members` is an array of user ids |
| POST | `/access/groups` | `access.manage` (MFA) on the workspace — `{name, description?}`: `name` 1–80 characters, unique ignoring case (`409` otherwise); `description` up to 500 characters, default empty. → `201` with the group and `members: []`. A new group grants nothing. Audited (`access.group.create`) |
| PATCH | `/access/groups/{id}` | `access.manage` (MFA) on the workspace — `{name?, description?}`: rename or describe; members and assignments stay. `409` when the name is taken, `404` for an unknown group. Audited (`access.group.update`) |
| DELETE | `/access/groups/{id}` | `access.manage` (MFA) on the workspace → `204`. Deletes the group, its memberships and its assignments in one transaction; every member loses access through it at once. Audited (`access.group.delete`, with `assignments_removed`) |
| PUT | `/access/groups/{id}/members/{user_id}` | `user.manage` on the workspace **and** an MFA-verified session → `204`; idempotent. You must hold every action the group holds over each scope where it is assigned (`403` `escalation`, naming the scope and the missing actions; the denial is audited). `404` for an unknown group or user. Audited (`access.group.member.add`) |
| DELETE | `/access/groups/{id}/members/{user_id}` | `user.manage` on the workspace; no MFA → `204`; idempotent. Access through the group ends at once. Audited (`access.group.member.remove`) |
| GET | `/access/effective?user_id=<id>&scope=<kind>:<id>` | `user.manage` on the scope, unless `user_id` is yourself. `scope` takes the assignable kinds (`422` otherwise; `404` when it does not exist). → `{user_id, scope, actions, sources}`: `actions` is every action as `{action, allowed}`, judged over the scope and everything beneath it as if the session were MFA-verified; `sources` lists the unexpired assignments on the scope or its ancestors that reach the user, as `{assignment_id, profile_id, profile_name, scope, via, group_id, group_name}` with `via` `direct` or `group`. Role bindings are counted in `actions` but not listed in `sources` |
| GET | `/service-accounts` | `view` on the workspace — `{id, name, description, disabled, owner_id, owner, owner_inactive, tokens, last_used_at, created_by, created_at}` |
| POST | `/service-accounts` | `access.manage` (MFA), not with an API key — `{name, description?, owner?}`: user `svc-<name>`, no sign-in factors. Audited (`service_account.create`) |
| PATCH | `/service-accounts/{id}` | `access.manage` (MFA) — `{disabled}`: disabling revokes all its tokens and ends its sessions and streams at once; enabling does not restore tokens. `404` for people. Audited (`service_account.disable` / `.enable`) |
| GET | `/service-accounts/{id}/tokens` | `view` on the workspace — tokens (no secrets) |
| POST | `/service-accounts/{id}/tokens` | `access.manage` (MFA), not with an API key — the same body as `POST /api-keys`, `restrictions` included: the token is a personal access token of the service account; the `secret` is in this answer only. `elevated` and `restrictions` follow the same rules as for `/api-keys`, measured against what the account holds. At most 10 tokens per account (`409`) and 20 per hour (`429`). Audited (`service_account.token.create`) |
| DELETE | `/service-accounts/{id}/tokens/{token}` | `user.manage` on the workspace; no MFA — revokes it and ends terminals opened with it. Audited (`service_account.token.revoke`) |
| GET | `/api-keys` | your own API keys (no secrets): `[{id, name, prefix, expires_at, last_used_at, created_at, elevated, restrictions}]`. `restrictions` is `null` for a key with all of your access, else `[{scope: {kind, id}, actions: [..]}]` |
| POST | `/api-keys` | signed in, not with an API key (`403`) — `{name, expires_days?, never_expires?, elevated?}`: a new key; the secret is in the response once. `expires_days` defaults to `90` (clamped to 1–3650); `never_expires: true` makes a key without expiry. `elevated: true` makes a key that counts as MFA-verified, so it can run MFA-guarded actions: it needs an MFA-verified session (`403` `step_up_required`, unless MFA is off) and `[security] allow_elevated_tokens` (`403` `elevated_tokens_disabled`). `restrictions?: [{scope: "<kind>:<id>", actions: [..]}]` makes a **restricted** key: 1–50 entries (`400` otherwise); `scope` is `workspace:`, `cluster:`, `project:`, `environment:`, `server:` or `service:<id>` and must exist (`404`); `actions` are names from `/access/catalog`. You must hold every listed action over each scope now (`403` `escalation`, naming the scope and the missing actions; the denial is audited). At every request the key may act only when you may **and** an entry lists the action at a scope that covers the resource; otherwise `403` with the detail `…: outside what this token was given`. Lists are filtered the same way. Omitted: the key acts with all of your access. Audited (`apikey.create`, with `token_id`, `elevated` and `restricted`, the number of entries or `null`) |
| DELETE | `/api-keys/{id}` | one of your own keys. Audited (`apikey.revoke`) |
| GET | `/users/{id}/api-keys` | `user.manage` on platform — another user's keys (no secrets) |
| DELETE | `/users/{id}/api-keys/{key}` | `user.manage` on platform — revokes one key; no MFA needed. Audited (`apikey.revoke`, `by_admin: true`) |
| DELETE | `/users/{id}/api-keys` | `user.manage` on platform — revokes all of the user's keys → `{revoked}`; no MFA needed. Audited (`apikey.revoke_all`) |
| DELETE | `/role-bindings/{id}` | `user.manage` on the binding's scope. No MFA needed: removing access is never held up |

`POST /role-bindings` takes `{"email": "<username-or-email>", "role": "<role>", "scope": "<scope>"}`
(or `user_id` instead of `email`). `scope` is `platform:root`,
`workspace:<id>`, `tenant:<id>`, `cluster:<id>`, `project:<id>` or
`environment:<id>`; any
other kind returns `400`. `platform_admin` is accepted only on
`platform:root`.

Creating accounts and granting roles need an MFA-verified session even
though `user.manage` itself is not under the MFA guardrail. Otherwise a
leaked API key or a session without MFA could create an account, grant it a
role and pass every MFA-guarded action with that account's own
authenticator. With MFA turned off for the server, every session counts as
verified.

### Environments

An environment belongs to one cluster and one project. Its name is free
text, unique within the cluster; the same name can exist in different
clusters.

**Environment object:**

| Field | Type | Notes |
| --- | --- | --- |
| `id` | string | Environment id |
| `workspace_id` | string | Owning workspace |
| `cluster_id` | string or null | Cluster the environment runs in. Always set for an environment created through the API |
| `project_id` | string or null | The project this environment belongs to. Always set for an environment created through the API |
| `name` | string | 1–100 characters, trimmed; unique within the cluster |
| `protected` | bool | Marks a production environment such as `prod`. Shown in the console; no guardrail enforces it yet |
| `created_at` | number | Epoch milliseconds |

**`GET /environments`** returns an array of environment objects the caller
can `view`. Each item is authorized on its own; others are left out.

| Query parameter | Type | Default | Notes |
| --- | --- | --- | --- |
| `cluster_id` | string | none | Only environments in this cluster |
| `project_id` | string | none | Only environments of this project |

**`POST /environments`** creates an environment of a project in a cluster.
Needs `workspace.manage` on the cluster and on the project. Audited as
`environment.create`.

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `name` | string | required | 1–100 characters, free text |
| `cluster_id` | string | required | Cluster to create the environment in |
| `project_id` | string | required | The project the environment belongs to |
| `protected` | bool | `false` | |

```http
POST /api/v1/environments
{"name": "test", "cluster_id": "<cluster-id>", "project_id": "<project-id>", "protected": false}
```

| Status | When |
| --- | --- |
| `201` | Created; returns the environment object |
| `400` | Invalid name, or `cluster_id` or `project_id` missing or unknown |
| `403` | `workspace.manage` missing on the cluster or the project |
| `409` | The cluster already has an environment with this name |

**`PUT /environments/{id}`** renames an environment. Needs
`workspace.manage` on the environment. Body `{"name": "<new name>"}`; `409`
when the cluster already has that name. Services keep running under their
old unit and container names until their next deploy. Audited as
`environment.rename`.

**`PUT /environments/{id}/project`** moves an environment to a project.
Needs `workspace.manage` on the environment and on the project. Body
`{"project_id": "<id>"}`; `400` when missing or unknown. Audited as
`environment.set_project`.

**`DELETE /environments/{id}`** removes an empty environment. Needs
`workspace.manage` on the environment. `409` while services or stacks still
exist in it: remove those first. Also `409` while the environment owns an
object store (`this environment owns <n> object store(s); assign them to
another environment or clear their owner first`): change or clear the owner
with [`PUT /object-stores/{id}/owner`](#object-storage). Audited as
`environment.delete`.

**`PUT /environments/{id}/cluster`** moves an environment, with its
services, into another cluster. Needs `workspace.manage` on the environment
and on the target cluster. Audited as `environment.move`.

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `cluster_id` | string | required | Target cluster. An environment cannot be moved out of every cluster |

| Status | When |
| --- | --- |
| `200` | Moved; returns the environment object |
| `400` | `cluster_id` missing or unknown |
| `403` | `workspace.manage` missing on the environment or the target cluster |
| `409` | The target cluster already has an environment with this name |

## Projects

A project is a product: a name that groups environments and, through them,
services. See
[Projects, environments and services](/docs/concepts/projects-environments-services/).

| Field | Type | Notes |
| --- | --- | --- |
| `id` | string | Project id |
| `workspace_id` | string | Owning workspace |
| `name` | string | 1–63 characters: lower-case letters, digits and dashes; unique within the workspace |
| `description` | string | Free text, may be empty |
| `created_at` | number | Epoch milliseconds |
| `environments` | array | `GET` only: `[{id, name, cluster_id, protected}]`, the environments of this project the caller may view |
| `service_count` | number | `GET` only: services across those environments |

**`GET /projects`** returns the projects the caller may `view`, each with
its environments. **`GET /projects/{id}`** returns one.

**`POST /projects`** creates a project. Needs `workspace.manage` on the
workspace. Audited as `project.create`.

```http
POST /api/v1/projects
{"name": "shop", "description": "Online shop: storefront, API and workers"}
```

| Status | When |
| --- | --- |
| `201` | Created; returns the project object |
| `400` | Invalid name |
| `403` | `workspace.manage` missing |
| `409` | A project with this name exists |

**`PUT /projects/{id}`** changes `name` and/or `description`
(`workspace.manage`, audited as `project.update`). **`DELETE
/projects/{id}`** removes a project with no environments (`204`; `409`
while environments remain; audited as `project.delete`).

Role bindings accept the scope `project:<id>`; a binding there covers every
environment of the project and their services.

## Servers

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/servers?workspace=&tag=` | `view` (filtered). Each node carries `facts` and `tags`. `tag` is repeatable or comma-separated; a node must carry every given tag (case-insensitive) |
| GET | `/servers/{id}` | `view` — includes `facts`, `tags` and `dns` (the node's [local name](#local-names)) |
| POST | `/enrollment-tokens` | `server.enroll` on the workspace, or on the environment when `environment_id` is set; with `cluster_id` also `server.enroll` on the cluster. Body below → `201` |
| POST | `/servers/{id}/revoke` | `server.manage` — `{confirm_hostname, totp_code?}`: the exact machine hostname, plus a fresh authenticator code when the caller has MFA enrolled. Refusals are audited |
| DELETE | `/servers/{id}` | `server.manage` — same body and confirmations as revoke → `204`. Revokes the agent certificate, then deletes the node and its data in one transaction: facts, saved logins, SSH and Docker settings, file roots, uploads and grants, installations, app instances, stack parts, deploy slots and events, object store memberships, tunnels and tunnel grants, observations, alarms, DNS label, tags, role bindings and access assignments scoped to `server:<id>`. Queued jobs on the node are cancelled. A stack whose default node it was moves to another of its nodes, or is deleted when it has none. The audit log and the revocation entry stay. The host name and DNS name are free afterwards. `400` hostname mismatch, `403` missing or wrong code, `404` unknown node, `409` the control node. Audited as `server.remove` with `hostname`, `display_name`, `cluster_id`, `cert_serial` |
| PUT | `/servers/{id}/name` | `server.rename` — `{name}` sets the console display name (empty restores the hostname); the machine hostname is unchanged. A node that has no local name yet takes it from this name; an existing local name does not change. Audited as `server.rename` |
| PUT | `/servers/{id}/dns-name` | `server.rename` — `{label}` sets the node's DNS label; empty goes back to a derived label that no other node has. Audited as `server.dns_rename`. [Details](#local-names) |
| GET | `/names?workspace=<id>` | `view` per node — the [local names](#local-names) of nodes and services on nodes you can see |
| GET | `/names/suffix` | `view` on the workspace — the suffix of every local name and where it comes from. [Details](#local-names) |
| PUT | `/names/suffix` | `system.update` on the platform, MFA-verified session — `{suffix}` sets the suffix of every local name; empty clears the runtime setting. Audited as `names.suffix.update`. [Details](#local-names) |
| GET | `/servers/{id}/metrics?since=<ms>` | `view` — in-memory samples newer than `since` and the node's mounted `volumes`; marks the node as watched (its agent then reports every few seconds) |
| GET | `/metrics/latest` | `view` per node — newest sample of each visible node |
| GET | `/servers/{id}/history?from=<ms>&to=<ms>` | `view` — the node's history from the analytics database: `{server_id, from, to, resolution, points}`. `resolution` is `minute` for windows up to 48 hours, `hour` beyond (at most 400 days). Defaults: the last 24 hours. `503 analytics_off` when `[analytics] engine = "off"`, `503 analytics_unavailable` when the analytics database cannot be reached |
| GET | `/servers/{id}/report?from=<ms>&to=<ms>` | `view` — the history plus `cpu_peak`, `mem_peak`, `disk_peak` (`{t, value}`, percent) and `top_cpu` / `top_mem`: the ten processes (`name`, `user`, `cpu_avg`, `cpu_max`, `mem_avg`, `mem_max`, `minutes`, `cmd`) that used the most in the window. Same errors as `history` |
| GET | `/analytics/status` | `view` on the platform — `{engine, ready, last_error, pending_rows, dropped_rows, retention, worker}`; `retention` holds the effective `node_metrics_days`, `top_processes_days`, `node_rollup_days` and `app_logs_days`; `worker` is the analytics worker process (see [`worker`](#the-worker-object)). While the worker is down, `ready` is `false` and `last_error` says why. Changing retention: [Analytics settings](#analytics-settings) |
| GET | `/servers/{id}/connectivity` | `view` — peers (cluster members + control node) and recent probe runs |
| POST | `/servers/{id}/connectivity` | `server.manage` — `{count?}` probes every peer *from this node* (ICMP, TCP-connect fallback) |
| POST | `/servers/{id}/commands` | `job.create` + `server.manage` (read-only) — `{command: {type: "ping" \| "collect_facts" \| "service_status", name?}}` → `202` job |
| GET | `/servers/{id}/logins` | `view` — `{saved: [string], discovered: [string], logins: [string]}`, the accounts offered on **Open terminal**. `saved` is the node's saved list; `discovered` holds the accounts the node reports in its facts (`login_accounts`) that are not saved, in the node's order; `logins` repeats `saved` for older clients |
| PUT | `/servers/{id}/logins` | `server.manage` — `{logins: [string]}` replaces the list; each 1–32 chars of `A-Za-z0-9_.-`, not starting with `-`; duplicates dropped; at most 32 (`400` otherwise). Audited as `server.logins.update`. Not a grant: the terminal still needs `terminal.open` |
| POST | `/servers/{id}/terminal-account` | `server.command` (MFA) — creates or keeps a terminal account on the node and saves it. Body and answer [below](#terminal-accounts) |
| GET | `/servers/{id}/processes?sort=&limit=&filter=` | `server.manage` — htop-like snapshot from the agent. `sort`: `cpu` (default), `mem`, `pid`, `time`, `name`; `limit` 1–500 (default `100`); `filter` case-insensitive substring of name, command or user (first 100 chars). `409` when the node is offline or refuses, `504` when it does not answer within 20 s. Not audited per call; denials are. Answer [below](#processes) |
| POST | `/servers/{id}/processes/{pid}/signal` | `process.manage` (MFA) — `{start_time, signal, tree?}` ends the process, or with `tree` also its descendants. Audited as `server.process.signal`. Body and answer [below](#processes) |
| POST | `/servers/{id}/processes/{pid}/priority` | `process.manage` (MFA) — `{start_time, nice?, class?}` sets the nice value (Linux) or priority class (Windows). Audited as `server.process.priority`. Body and answer [below](#processes) |

**Node `facts`** (`GET /servers`, `GET /servers/{id}`): an object with any of
`hostname`, `os`, `os_version`, `kernel`, `arch`, `cpus`, `agent_version`,
`address` (where the agent connects from), `ssh_principals` (boolean: sshd
accepts only node-bound certificates),
`login_accounts` (array of strings: the accounts a terminal can log in as,
at most 64; empty on Windows), `collected_at_ms` (epoch ms), and
from the latest live sample `mem_total_bytes` and `disk_total_bytes`. Collected
when the node connects and daily; `{}` until the first collection.

### Local names

Every node keeps the local names of its workspace in its hosts file
([concept](/docs/concepts/local-names/)). Names end in the suffix: the runtime
setting if set, else `[names] suffix`, else `local`.

`PUT /api/v1/servers/{id}/dns-name` takes `{"label": "<label>"}`:

| Field | Type | Rules |
| --- | --- | --- |
| `label` | string | 1–63 characters of `a-z`, `0-9` and `-`, not starting or ending with `-`. Not `localhost`, `local`, `localdomain` or another reserved name. Unique in the workspace. An empty string clears it |

Answers `{server_id, label, label_set, name}`, for example
`{"label": "db", "label_set": true, "name": "db.local"}`. `400` for an
invalid label, `409` when another node of the workspace has it (a set or a
derived label). Every node
receives the new name within one sync pass (30 s).

`GET /api/v1/names` answers:

| Field | Type | Description |
| --- | --- | --- |
| `enabled` | bool | `[names] enabled` |
| `suffix` | string | The suffix in use |
| `suffix_source` | string | `setting`, `config` or `default` |
| `names[]` | array | One row per name and node: `name`, `kind` (`node` or `service`), `server_id`, `node` (its label), `cluster_id?`, `address?` (the address the server sees the node connect from), `mesh_address?` (its fabric address), `app?` (`<environment>/<app>`), `ports[]` (`{port, proto, target, bind?}`: ports the service publishes on the node), `workspace_id` |
| `nodes[]` | array | Per node: `server_id`, `label`, `label_set`, `name`, `sync` (`{names, in_sync, synced_at_ms?, error?}`, or `null` before the first pass) |

Nodes in the same fabric get each other's mesh addresses; every other
node gets `address`.

`GET /api/v1/names/suffix` (`view` on the workspace) and
`PUT /api/v1/names/suffix` answer:

| Field | Type | Description |
| --- | --- | --- |
| `suffix` | string | The suffix in use |
| `source` | string | `setting` (the runtime setting), `config` (`[names] suffix` or `FORGE_NAMES_SUFFIX`) or `default` |
| `config_suffix` | string | The config file's value: what clearing the setting goes back to |
| `default_suffix` | string | `local` |

`PUT /api/v1/names/suffix` takes `{"suffix": "<suffix>"}`:

| Field | Type | Rules |
| --- | --- | --- |
| `suffix` | string | Lowercase DNS labels separated by dots, each 1–63 characters of `a-z`, `0-9` and `-`, not starting or ending with `-`; at most 63 characters in all. One label only from `local`, `internal`, `lan`, `home`, `test`, `localdomain`, `intranet`, `corp`. No `localhost` label. An empty string clears the runtime setting |

```http
PUT /api/v1/names/suffix
Content-Type: application/json

{"suffix": "home.arpa"}
```

```json
{"suffix": "home.arpa", "source": "setting", "config_suffix": "local", "default_suffix": "local"}
```

Needs `system.update` on the platform (**Platform admin**). `403` with
`step_up_required` when MFA is required and the session is not
MFA-verified, `403` without the permission, `400` for an invalid suffix
(audited as denied). Every node gets its list with the new names on the
next pass (30 s); the old names stop resolving. Audited as
`names.suffix.update` with `from`, `from_source`, `to` and `source`.

### Processes

`GET /api/v1/servers/{id}/processes` answers with a snapshot of the node:

| Field | Type | Notes |
| --- | --- | --- |
| `summary.os` | string | `linux` or `windows` |
| `summary.cpus` | number array | Usage per core, in percent |
| `summary.cpu_total` | number | Usage of all cores, in percent |
| `summary.load` | number array or `null` | Linux: load average over 1, 5 and 15 minutes. Windows: `null`, because Windows has no load average |
| `summary.mem_total`, `summary.mem_used` | integer | Memory, in bytes |
| `summary.swap_total`, `summary.swap_used` | integer | Swap, in bytes |
| `summary.tasks` | integer | Processes on the node |
| `summary.running` | integer | Processes in the running state |
| `summary.uptime` | integer | Seconds since the node booted |
| `actions` | object | What the node can do to a process; see below |
| `matched` | integer | Processes that match `filter`, before `limit` |
| `processes` | array | The top processes, sorted by `sort`; fields below |

Each entry of `processes`:

| Field | Type | Notes |
| --- | --- | --- |
| `pid` | integer | Process id |
| `ppid` | integer or `null` | Parent process id |
| `user` | string | Linux: the owner's name from `/etc/passwd`, else the uid. Windows: `NT AUTHORITY\SYSTEM`, `NT AUTHORITY\LOCAL SERVICE`, `NT AUTHORITY\NETWORK SERVICE`, `Window Manager\DWM-<n>`, `Font Driver Host\UMFD-<n>` or a local account name; any other owner, such as a domain account, as its SID (`S-1-5-21-…`) |
| `state` | string or `null` | Linux: the htop letter `R`, `S`, `D`, `Z`, `T`, `X` or `?`. Windows: `null` |
| `nice` | integer or `null` | Linux only: the nice value, `-20` to `19`; `null` when it cannot be read. Not present on Windows: Forge does not read priority classes |
| `cpu` | number | CPU usage since the previous snapshot, in percent of one core |
| `mem` | integer | Resident memory, in bytes |
| `virt` | integer | Virtual memory, in bytes |
| `mem_pct` | number | Resident memory, in percent of `summary.mem_total` |
| `time` | integer | Seconds since the process started |
| `started` | integer | Start time, in seconds since the epoch. Pass it as `start_time` to the actions below |
| `name` | string | Process name |
| `cmd` | string | Command line, at most 300 characters |

`actions` per OS:

| Field | Linux | Windows |
| --- | --- | --- |
| `signals` | `["term", "kill", "hup", "int"]` | `["term", "kill"]` |
| `tree` | `true` | `true` |
| `nice` | `true` | `false` |
| `priority_classes` | `[]` | `["idle", "below_normal", "normal", "above_normal", "high"]` |

**`POST /api/v1/servers/{id}/processes/{pid}/signal`** ends a process:

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `start_time` | integer | required | The process's `started` value from the list. The node refuses when the PID now belongs to another process |
| `signal` | string | required | Linux: `term` (`SIGTERM`), `kill` (`SIGKILL`), `hup` (`SIGHUP`), `int` (`SIGINT`). Windows: `term` (End task: `taskkill` without `/F`, asks the process to close) or `kill` (End process: `taskkill /F`, forced) |
| `tree` | boolean | `false` | Also the process's descendants: the process first, then its children and their children, as listed before acting |

```json
{"start_time": 1794589874, "signal": "term", "tree": false}
```

On Windows, the agent runs as a service in session 0. `term` reaches only
processes with a window in that session, so desktop apps and console
programs usually refuse with "This process can only be terminated
forcefully"; Forge returns that message. Use `kill` for them.

**`POST /api/v1/servers/{id}/processes/{pid}/priority`** changes a process's
priority:

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `start_time` | integer | required | The process's `started` value from the list |
| `nice` | integer | none | Linux: required. `-20` (highest priority) to `19` |
| `class` | string | none | Windows: required. `idle`, `below_normal`, `normal`, `above_normal` or `high`. `realtime` is never allowed |

```json
{"start_time": 1794589874, "nice": 10}
```

On Windows the node sets the class through the built-in Windows PowerShell,
which takes about half a second.

Both bodies refuse unknown fields. `200` answer:

| Field | Type | Notes |
| --- | --- | --- |
| `pid` | integer | The process from the path |
| `action` | object | What the node ran: `kind` (`signal`, `nice` or `priority_class`), `pid`, `start_time` and `signal` with `tree`, `nice`, or `class` |
| `affected` | integer array | The PIDs the node acted on, in order |
| `exited` | integer array | Descendants that exited before Forge reached them |

```json
{
  "pid": 4242,
  "action": {"kind": "signal", "pid": 4242, "start_time": 1794589874, "signal": "term", "tree": true},
  "affected": [4250, 4243, 4242],
  "exited": []
}
```

| Status | When |
| --- | --- |
| `400` | PID `0`; a signal the node's OS does not have; no `nice` on a Linux node or no `class` on a Windows node; `nice` out of range or an unknown `class` |
| `403` | No `process.manage` on the node, or `step_up_required` without an MFA-verified session |
| `404` | Unknown node |
| `409` | `the node is offline`; `the node refused: <reason>` (see below); or the node's command queue is full |
| `422` | An unknown field, a missing required field or a wrong type in the body |
| `504` | `the node did not answer in time`: no answer within 30 seconds |

The node checks, before it acts, that the PID still belongs to the process
that started at `start_time`, and refuses otherwise ("process `<pid>` is no
longer the one listed: it exited and the PID was reused (refresh the
list)"). On Linux it pins the process with a pidfd before the check, so a
reused PID never gets the signal. It also refuses protected processes, and
checks the whole tree before it acts on any of them:

| OS | Protected |
| --- | --- |
| Both | PID `0`; every Orbyn Forge component (process names `forge` and `forge-*`, such as `forge-agent`, its helper and a co-located `forge-server`) |
| Linux | PID `1` (init), PID `2` and kernel threads (children of PID `2`) |
| Windows | PID `4`, `System`, `Registry`, `Secure System`, `Memory Compression`, `smss`, `csrss`, `wininit`, `services`, `lsass`, `LsaIso`, `winlogon` |

Other reasons for a refusal: the process exited before Forge acted, the OS
denied the change, or the agent's `allowed_commands` lacks
`process_action` (also on agents older than the control plane).

On Linux the node's root helper runs the action, derived only from the
signed command. On Windows the agent (LocalSystem) runs
`%SystemRoot%\System32\taskkill.exe` or
`%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe` by absolute
path, with fixed arguments: only integers and a class name from the list
above.

Audited as `server.process.signal` (`pid`, `start_time`, `signal`, `tree`)
or `server.process.priority` (`pid`, `start_time`, and `nice` or `class`),
with `affected` on success. When the node refuses or does not answer, the
entry has the outcome `failure` and an `error`. The node runs the command
kind `process_action`, which is in the agent's default allowlist.

### Terminal accounts

`POST /api/v1/servers/{id}/terminal-account` has the node's root helper
create a local account for the browser terminal, or keep it when a regular
user of that name exists, and set its passwordless `sudo` rule. Forge then
adds the account to the node's saved logins. Linux nodes only.

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `name` | string | required | 1–32 characters of `a-z`, `0-9`, `_` and `-`, starting with a letter or `_`. Not a reserved system name (`root`, `nobody`, `forge-agent`, `daemon`, `bin`, `sys`, `sync`, `games`, `man`, `lp`, `mail`, `news`, `uucp`, `proxy`, `www-data`, `backup`, `list`, `irc`, `gnats`, `sshd`, `messagebus`, `admin`, `adm`, `sudo`, `wheel`, `docker`, `polkitd`, `halt`, `shutdown`, `operator`) and not starting with `systemd-`. Leading and trailing spaces are trimmed |
| `sudo` | boolean | `false` | `true` writes `/etc/sudoers.d/forge-<name>` with `<name> ALL=(ALL:ALL) NOPASSWD: ALL`, checked with `visudo -c`; `false` removes that file when Forge wrote it |

Unknown fields are refused. Example:

```json
{"name": "forge-ops", "sudo": false}
```

`200` answer:

| Field | Type | Notes |
| --- | --- | --- |
| `name` | string | The account |
| `sudo` | boolean | As requested |
| `created` | boolean | `true` when the node created the user; `false` when it existed |
| `logins` | string array | The node's saved logins after the change |

| Status | When |
| --- | --- |
| `400` | The name breaks the rules above |
| `403` | No `server.command` on the node, or `step_up_required` without an MFA-verified session |
| `404` | Unknown node |
| `409` | `terminal accounts need a Linux node`; `the node refused: <reason>` (the name is a system user or an existing group there, `sudo` is missing, `visudo` rejected the rule, or the agent's `allowed_commands` lacks `terminal_account`); or the node's command queue is full |
| `504` | `the node did not answer in time`: no answer within 60 seconds, for example because the node is offline. The signed command stays valid for 120 seconds |

Audited as `server.terminal_account.create` with `name`, `sudo` and
`created`. When the node refuses, the entry has the outcome `failure` and an
`error`. When the saved list changes, a `server.logins.update` entry with
`from` and `to` follows. The node runs the command kind `terminal_account`,
which is in the agent's default allowlist.

Forge also saves logins without this endpoint, audited as
`server.logins.update`:

| Source | Saved login | Recorded as | `source` in the entry |
| --- | --- | --- | --- |
| Assisted onboarding (`POST /nodes/onboard`) | The SSH `username`, once the node is online | The person who started the onboarding | `onboarding` |
| Installer enrollment with `terminal_account` | The account the installer created | The person who created the enrollment token | `installer` |

A login is saved only when it is valid, not saved yet, and the list holds
fewer than 32.

### Container telemetry

Logs and per-minute metrics of every Docker container on a node. See
[Container log collection](/docs/concepts/container-logs/). Container log
lines are read through [`GET /logs`](#service-logs) with `source=container`.

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/servers/{id}/container-metrics?from=&to=&limit=` | `view` on the node → `{server_id, from, to, resolution, containers}`. `from` and `to` are epoch ms (default: the last 24 hours); `limit` is the most containers returned, by name, 1–200 (default 200) |
| GET | `/servers/{id}/container-metrics/{name}?from=&to=` | `view` on the node → the same body for one container. `400` when `name` is empty, longer than 256 characters or has control characters |
| GET | `/servers/{id}/container-telemetry` | `view` on the node → `{enabled, exclude}` |
| PUT | `/servers/{id}/container-telemetry` | `server.manage` on the node — `{enabled, exclude}` → the saved settings. Audited as `server.container_telemetry.update` with `from` and `to` |

`resolution` is `minute` for windows up to two days and `hour` beyond. Each
element of `containers` is `{container_name, container_id, image,
service_id, stack, points}`; `service_id` and `stack` are `null` when the
container belongs to none. Each point is `{t, cpu_avg, cpu_max, mem_used,
mem_limit, rx, tx, block_read, block_write, restarts}`: `t` epoch ms, CPU in
percent, memory in bytes, network and block I/O in bytes per second.

**Settings body:**

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Collect logs and metrics of every container on the node |
| `exclude` | list of strings | `[]` | Containers left out: a container name, or `key=value` to match a label. At most 64 entries of up to 256 characters (`400` otherwise) |

The history endpoints answer `503` `analytics_off` when analytics is off,
`503` `analytics_worker_down` while the analytics worker restarts, and
`503` `analytics_unavailable` when the engine cannot be reached.

## Tags

Tags on nodes, clusters, environments and services. `{kind}` is `server`,
`cluster`, `environment` or `service`.

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/tags?kind=` | `view` per item — `[{tag, count}]` over the items you can see, sorted |
| GET | `/tags/{kind}` | `view` per item — `{"<id>": [tags]}` for the items you can see |
| GET | `/tags/{kind}/{id}` | `view` — `{tags}` |
| PUT | `/tags/{kind}/{id}` | `server.manage` (node), `workspace.manage` (cluster, environment) or `deploy` on the environment (service) — `{tags: [string]}` replaces the set; returns the normalised set. Audited as `tags.update` with `from` and `to` |

A tag is a plain word or `key:value` with both parts non-empty; at most 64
characters, no commas or control characters, at most 50 per item. Whitespace
is trimmed and duplicates are dropped case-insensitively. Invalid sets return
`400`.

**Enrollment token body** (`POST /enrollment-tokens`):

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `environment_id` | string | none | Environment the node is registered in; must belong to the caller's workspace (`400` otherwise) |
| `ttl_minutes` | number | `60` | Validity in minutes, clamped to 1–1440 |
| `cluster_id` | string | none | Cluster the node joins when it enrolls with this token; the new member gets a cluster-network address. Must belong to the caller's workspace (`400` otherwise) |
| `hostname` | string | none | Expected host name, shown in `GET /onboarding/nodes` until the node enrolls. Display only: the node registers with its own hostname. Ignored when empty or longer than 253 characters |

Response `201 {id, token, expires_at, install: {linux, windows}}`. `token`
is shown once; only its hash is stored. `install` holds a command for each
OS. Both prompt for the token (`Enrollment token`) instead of containing it,
so it stays out of shell history and process lists:

| Field | Command |
| --- | --- |
| `install.linux` | Reads the token without echo, then runs `forge-agent enroll --server <agent_public_url>` and unsets the variable. It only enrolls an agent that is already installed; the **Add node** page builds the `install.sh` command itself |
| `install.windows` | Reads the token masked (`Read-Host -AsSecureString`) into `$env:FORGE_ENROLL_TOKEN`, then runs `irm <forge-url>/install.ps1 \| iex`. The installer removes the variable when it is done. See [Node installer: Windows](/docs/reference/cli/node-installer/#windows-installps1) |

`<forge-url>` is `node_public_url`, or `public_url` when that is not set.
Audited as `server.enrollment_token.create`.

## Observations

Health evidence about nodes and services (D-085). See
[Health observations](/docs/concepts/health-observations/). Read-only; needs
`health.view` on the resource.

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/observations?resource=<kind>:<id>` | `health.view` — `{now, observations: [...]}`, current values of one resource |
| GET | `/observations/history?resource=<kind>:<id>&subject=<subject>` | `health.view` — `{key, entries: [...]}`, stored history of one key, newest first |

**Query parameters:**

| Parameter | Type | Default | Notes |
| --- | --- | --- | --- |
| `resource` | `<kind>:<id>` | required | `kind` is `server`, `service` or `cluster`; anything else returns `400` |
| `server` | node id | none | Only instances on this node |
| `subject_prefix` | string | none | `/observations` only. Subjects starting with this, e.g. `health.` |
| `subject` | string | required | `/observations/history` only. Lowercase dotted name (`a-z`, `0-9`, `.`, `-`, `_`, at most 128 bytes); `400` otherwise |
| `since` | epoch ms | `0` | `/observations/history` only. Entries received at or after this time |
| `limit` | number | `200` | Clamped to 1–1000 |

**Observation fields** (`observations[]`):

| Field | Type | Notes |
| --- | --- | --- |
| `key` | object | `{resource, server?, subject}`; `resource` is `{kind, id}` |
| `value` | object | Last reported value, e.g. `{"type": "health", "state": "failing"}` or `{"type": "startup", "state": "waiting"}`, or a measurement `{"type": "measure", "state": {"milli": 85250, "unit": "percent"}}` (thousandths of the unit; `milli` is `null` when unknown; units `percent`, `bytes`, `seconds`, `count`) |
| `effective` | object | The value to act on now: `value` while fresh, the same type with state `unknown` after |
| `stale` | boolean | `true` once `now` is past `fresh_until` |
| `reason` | string or null | `app_failure`, `timeout`, `connect_failed`, `tls`, `auth`, `invalid_config`, `unsupported_executor`, `location_unavailable`, `permission_denied` or `stale` |
| `detail` | string or null | Sanitized diagnostic text, at most 512 characters |
| `source` | object | `{"kind": "probe", "policy", "check"}`, `{"kind": "runtime", "runtime"}`, `{"kind": "agent"}` or `{"kind": "control_plane"}` |
| `executor` | node id or null | The node that ran the check |
| `generation` | string or null | Workload instance generation, e.g. a container id |
| `observed_at` | epoch ms | Executor time, clamped to within 30 s of `received_at` |
| `received_at` | epoch ms | Server time the value arrived |
| `fresh_until` | epoch ms | `received_at` plus the reported freshness (1 s to 1 h) |
| `skewed` | boolean | The executor's clock was more than 30 s off |
| `changed_at` | epoch ms | When the value last changed |

**History entry fields** (`entries[]`): `value`, `reason`, `detail`,
`generation`, `observed_at`, `received_at` and `skewed`, as above.

Example:

```sh
curl -s -H "Authorization: Bearer <api-key>" \
  "https://<forge-host>/api/v1/observations?resource=server:<node-id>&subject_prefix=health."
```

```json
{
  "now": 1763120430000,
  "observations": [
    {
      "key": { "resource": { "kind": "server", "id": "<node-id>" }, "subject": "health.readiness.web" },
      "value": { "type": "health", "state": "failing" },
      "effective": { "type": "health", "state": "failing" },
      "stale": false,
      "reason": "timeout",
      "detail": "connect timed out after 2s",
      "source": { "kind": "probe", "policy": "p1", "check": "web" },
      "executor": "<node-id>",
      "generation": "g1",
      "observed_at": 1763120420000,
      "received_at": 1763120420000,
      "fresh_until": 1763120480000,
      "skewed": false,
      "changed_at": 1763120420000
    }
  ]
}
```

## Agent updates

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/agents/versions` | `view` (filtered) — `{target, server, outdated, nodes: [{id, hostname, display_name, status, os, arch, agent_version, outdated, ahead, updatable, verdict, reasons, warnings}]}`. `target` is the version of the agent binaries on offer (`MANIFEST.json` in `agent_dist_dir`, else the server's version); `server` is the server's own version; revoked nodes are left out. `outdated`: the agent runs an older version than `target`; `ahead`: a newer one (never updated or rolled back). A release candidate or dev build of `X.Y.Z` is older than `X.Y.Z`. `updatable` is `true` for outdated Linux nodes. `verdict` says what an update would do: `will_update` (outdated, online, and the node reports it can update itself), `unknown` (outdated and online, from an agent that does not report it; still updated), `needs_reinstall` (see `reasons`), `offline`, `ahead` or `current`. `reasons` and `warnings` are lists of `not_linux`, `no_agent_build`, `no_systemd`, `no_curl`, `bin_dir_not_writable`, `low_disk`, `ephemeral_container` (a warning: the update lasts until the container is recreated) or `unknown` (named by a newer agent). Selecting "every outdated node" (an empty `server_ids` below) takes the `will_update` and `unknown` nodes; a node you name is always sent the update, and its own refusal decides |
| POST | `/servers/{id}/agent/update` | `server.command` (MFA) → `202` `agent.update` job; `404` unknown node; `409` revoked node; `409` when another update of the node is queued or running (audited as a denied `agent.update`) |
| POST | `/agents/update` | `server.command` (MFA) per node — `{server_ids?: [string]}` → `202 {target, jobs: [{server_id, job_id}], refused: [{server_id, error}]}`. An empty or missing `server_ids` selects every online, outdated Linux node. At most 500 nodes (`400` otherwise). A node with another update queued or running lands in `refused` |
| POST | `/agents/rollouts` | `server.command` (MFA) on every node, checked before anything starts, and `job.create` and `view` on the platform — `{server_ids?: [string], batch: number}` → `202 {job, target, nodes, batches}`; `409` while another rollout is queued or running. See [Rollouts](#rollouts) |
| GET | `/agents/auto-update` | `view` on the workspace → the [automatic update settings](#automatic-updates) |
| PUT | `/agents/auto-update` | `system.update` on the platform, and an MFA-verified session (`403` `step_up_required` otherwise) → the saved settings. Audited as `agent.auto_update.update` on the platform, denials too |

Each `agent.update` job has the steps `send` and `await`, is re-authorized
as `server.command` on every step, and has at most 3 attempts. A single
update (`/servers/{id}/agent/update` or `/agents/update`) returns the queued
or running update of the same node and target version instead of starting a
second one. A node never has two updates in flight: a single update is
refused (`409`) while a rollout's update of that node is queued or running,
and a rollout stops when a node already has another update in flight. After
an update failed or was cancelled, a new request starts a new job. A node already on the target version, or a newer one, is skipped. Windows nodes fail
with a fatal error. See [Update agents](/docs/guides/update-agents/).

### Forge self-update

| Method | Path | Notes |
| --- | --- | --- |
| GET | `/system/update` | signed in → `{current, latest, available, checked_at, state, message, enabled, can_update, target, feed_url}`. `state` is `idle`, `downloading`, `verifying`, `installing`, `restarting` or `failed` |
| POST | `/system/update/check` | `system.update` on the platform (MFA) — asks the feed now → the same object |
| GET | `/system/update/releases` | `view` on the platform → `{current, releases: [{version, published_at, notes, url, current}]}`: releases newer than or equal to the running version, newest first. Older versions are never listed. Drafts and prereleases are left out unless the server runs a prerelease. `400` when the feed cannot be reached |
| POST | `/system/update/apply` | `system.update` on the platform (MFA) — `{version, agents?, batch?}` → `202 {version, agents, state: "downloading"}`. `version` is any listed release strictly newer than the running one (`1.0.0` is newer than `1.0.0-rc-01`); Forge never downgrades (`409`, audited as denied). `agents` is `rollout` (default) or `none`; `batch` is 1–100 (default `1`). Audited as `system.update` with `from`, `to`, `agents` and `batch` |

The server fetches the release by tag (`<releases_url>/tags/v<version>`),
then checks its signature and checksums, test-runs the new binary
(`--version`, `config` and `check-migrations`), backs up a SQLite database,
swaps the binaries and restarts. With `agents: "rollout"`, the new server
waits for the agents to reconnect (up to three minutes), then submits one
`agent.rollout` of every outdated online Linux agent to its own version, as
the person who applied the update. That needs `server.command` on every
node. The audit log records `system.update.agents` as `system:forge`, with
the job, or with the error when the rollout was refused.

### Rollouts

`POST /agents/rollouts` updates nodes in batches. The first batch is the
canary.

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `server_ids` | array of string | `[]` | Nodes in the order they are updated. Empty or missing: every online, outdated Linux node. At most 500 (`400` otherwise) |
| `batch` | integer | required | Nodes per batch, `1` to `100` (`400` otherwise) |

Response `202`:

| Field | Type | Notes |
| --- | --- | --- |
| `job` | object | The `agent.rollout` job |
| `target` | string | The version the nodes are updated to (the agents on offer, see `GET /agents/versions`) |
| `nodes` | integer | Number of nodes in the rollout |
| `batches` | integer | Number of batches |

Errors: `403` when you lack `server.command` (MFA) on any node (the message
gives the number of refused nodes), or `job.create` or `view` on the
platform; `409` when no node is selected (nothing outdated), when
`[jobs] concurrency` is less than `2`, or when another `agent.rollout` job is
queued or running. A rollout is audited as `agent.rollout` with `nodes`,
`batch`, `batches` and `target`; a refused rollout is audited as a denied
`agent.rollout`.

The `agent.rollout` job has the resource `platform:root` and no workspace,
so only roles bound on the platform see it (`view`) or cancel it
(`job.cancel`). It has one step, `roll`, and one attempt. Only one rollout
is queued or running at a time. For each node of a batch it submits an
ordinary `agent.update` job as the rollout's principal, in that node's
workspace, with `rollout` (the rollout's job id) in its payload and the
idempotency key `agent.update:<server-id>:<target>:<rollout-id>`, and
`target` and `deadline_ms` (the end of the batch,
`[update] agent_batch_timeout_secs` after it starts) in its payload. The
rollout submits a node's update only once the node is online, and skips a
node still offline at `deadline_ms` without a job. The child update
**skips** a node that is not Linux, does not answer by `deadline_ms`, or
refuses the update with the `error_code` `unsupported` or `downgrade` (see [Agent protocol](/docs/reference/agent-protocol/#command-results)).
A skipped node does not stop the rollout. The next batch starts when every
node of the batch succeeded or was skipped. The job fails with
`stopped in batch k of n: node <id> failed (…); later nodes were not touched`
(or `… was refused …`, also when the node already has another update queued
or running) at the first real failure, and with
`batch k of n did not finish in <m> minutes; later nodes were not touched`
when a batch runs past its timeout plus 10 minutes. A batch whose nodes were
all skipped tested nothing, so the next batch is then the canary.
Cancelling the rollout stops its wait, and child updates that have not sent
their command yet fail with `the rollout was cancelled`. A later rollout can retry nodes that failed or were skipped.

The job's state:

| Key | Type | Notes |
| --- | --- | --- |
| `target` | string | Target version |
| `batches` | integer | Number of batches |
| `done_batches` | integer | Batches that finished |
| `skipped` | integer | Nodes skipped so far |
| `nodes` | array | `{server_id, job_id, status, outcome}` per node of the finished or stopped batches. `outcome` is `succeeded`, `skipped` (with `skip_reason` `offline`, `unsupported` or `downgrade`, and `reason`; an offline node skipped by the rollout has no `job_id`) or `failed`. `{server_id, status: "refused", error}` for a node whose update job could not be queued |

### Automatic updates

`GET` and `PUT /agents/auto-update` read and change automatic updates. They
are off by default. `PUT` takes `enabled`, `days`, `start`, `end` and
`batch`; unknown fields are refused (`400`).

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `enabled` | boolean | `false` | Required in `PUT`. Saving with `true` makes the caller the person the rollouts run as |
| `days` | array of string | `[]` | `mon`, `tue`, `wed`, `thu`, `fri`, `sat`, `sun`. Empty: every day. Other values: `400` |
| `start` | string | `"02:00"` | Window start, `HH:MM` UTC |
| `end` | string | `"05:00"` | Window end, `HH:MM` UTC, exclusive. Must differ from `start` (`400`). An end before the start is past midnight; the window belongs to the day it starts |
| `batch` | integer | `1` | Nodes per batch, `1` to `100` |
| `timezone` | string | `"UTC"` | Response only |
| `enabled_by` | string or null | `null` | Response only. Principal (`user:<id>`) the rollouts run as; `null` when off |
| `enabled_at` | integer or null | `null` | Response only. When `enabled_by` saved the setting with MFA, epoch ms |
| `expires_at` | integer or null | `null` | Response only. `enabled_at` plus 90 days, epoch ms. After it, automatic rollouts are refused until the setting is saved again |
| `last_target` | string or null | `null` | Response only. Last version an automatic rollout was started for |
| `last_job` | string or null | `null` | Response only. Id of that `agent.rollout` job |
| `last_error` | string or null | `null` | Response only. Why the last automatic rollout could not start |

The server checks every 5 minutes. When automatic updates are on, the time
is inside the window, outdated online Linux nodes exist, no `agent.rollout`
job is queued or running and `last_target` is not the current version, it
checks that `enabled_by` still exists, is not disabled and saved the setting
less than 90 days ago. Then it starts one rollout over every outdated online
Linux node as `enabled_by`, with the checks of `POST /agents/rollouts`. It
records the version in `last_target` only after the rollout started, so each
version gets one rollout; a refused or failed start is tried again on the
next check in the window.

Audit actions, actor `system:forge`, resource `platform:root`. Each has
`as` (the `enabled_by` principal) and `target` in its details:

| Action | Outcome | Details |
| --- | --- | --- |
| `agent.auto_update.start` | `success` | `job`, `nodes`, `batch` |
| `agent.auto_update.refused` | `denied` | `reason`: `enabled_by` is disabled or deleted, or the setting is older than 90 days |
| `agent.auto_update.node_refused` | `denied` | `server_id`: one entry per node `enabled_by` may not command |
| `agent.auto_update.failed` | `denied` (authorization) or `failure` | `error` |

The settings are stored under the key `agents.auto_update`. The generic
settings endpoints `PUT /settings/{key}` and `DELETE /settings/{key}` refuse
every key that starts with `agents.` with `400`.

## Server settings

Server-level settings, shown in the console under **Settings → Server**. See
[Public address](/docs/reference/configuration/#public-address) and
[Allowed hosts](/docs/reference/configuration/#allowed-hosts).

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/server/settings` | `view` on the platform (a platform-scope role) — `{public_address, allowed_hosts, config}` |
| PUT | `/server/public-address` | `system.update` on the platform, MFA-verified session — `{value}` pins the public address. Audited as `server.public_address.update` (refusals as `denied`) |
| DELETE | `/server/public-address` | Same — clears it; background links use `public_url`, else the last administrator address. Audited as `server.public_address.clear` |
| PUT | `/server/allowed-hosts` | `system.update` on the platform, MFA-verified session — `{hosts: [string]}` replaces the allowed host patterns. Applies at once. Audited as `server.allowed_hosts.update` with `from` and `to` (refusals as `denied`) |
| DELETE | `/server/allowed-hosts` | Same — back to `["*"]`, any host. Audited as `server.allowed_hosts.clear` |

`PUT` and `DELETE /server/public-address` answer with the `public_address`
object:

| Field | Type | Description |
| --- | --- | --- |
| `effective` | string | The address background links (alerts, OIDC) use now, without a trailing `/`; empty when none is known yet. Links in the console use the address you are on |
| `source` | `setting`, `config`, `last_admin` or `none` | Where `effective` comes from: the runtime setting, `public_url`, the address an administrator last signed in on, or nothing yet |
| `setting` | string or `null` | The runtime setting |
| `configured` | string or `null` | `public_url` from `forge.toml` |
| `last_admin` | string or `null` | The address a platform administrator last signed in on; recorded automatically, read-only |
| `https` | boolean | Whether `effective` uses `https` |
| `allowed_origins` | string[] | Browser origins the WebSocket endpoints and `/mcp` accept besides the request's own origin on an allowed host |

`PUT` and `DELETE /server/allowed-hosts` answer with the `allowed_hosts`
object:

| Field | Type | Description |
| --- | --- | --- |
| `hosts` | string[] | The patterns |
| `any` | boolean | Whether `*` is in the list |

`config` holds `public_url` (string or `null`), `node_public_url`,
`agent_public_url`, `cookie_secure` and `trusted_proxies` from `forge.toml`,
read-only.

`value` must be an `http` or `https` origin, at most 255 characters, without a
path, query, fragment, user name or password. Anything else is `400`.

`hosts` holds 1 to 64 patterns: `*`, a host name, an IP (IPv6 in brackets),
or `*.<domain>` where `<domain>` has at least two labels (`*.com` is
refused). Duplicates are dropped. A pattern with a port, a path, a space, or
a wildcard anywhere but the first label is `400` `hosts: <reason>`.

The generic `PUT /settings/{key}` and `DELETE /settings/{key}` refuse keys
that start with `server.` with `400`, and keys that start with `agent_tls.`
with `400` `the agent key is managed with forge-server agent-key`. `GET
/settings` does not list `agent_tls.` keys. There is no REST endpoint for the
control-plane key; use [`forge-server agent-key`](/docs/reference/cli/forge-server/#forge-server-agent-key)
on the server host.

Every endpoint except `GET /api/health` answers `421 Misdirected Request`,
with no body, when the request's host is not allowed, and `400` when the
`Host` (or a trusted `X-Forwarded-Host`) is malformed.

```sh
curl -sS -X PUT https://<forge-host>/api/v1/server/public-address \
  -H "Authorization: Bearer $FORGE_TOKEN" -H 'Content-Type: application/json' \
  -d '{"value":"https://forge.example.com"}'
```

```json
{"effective":"https://forge.example.com","source":"setting","setting":"https://forge.example.com","configured":null,"last_admin":"http://192.0.2.10:3880","https":true,"allowed_origins":["https://forge.example.com"]}
```

```sh
curl -sS -X PUT https://<forge-host>/api/v1/server/allowed-hosts \
  -H "Authorization: Bearer $FORGE_TOKEN" -H 'Content-Type: application/json' \
  -d '{"hosts":["forge.example.com","192.0.2.10","localhost"]}'
```

```json
{"hosts":["forge.example.com","192.0.2.10","localhost"],"any":false}
```

## Analytics settings

Runtime analytics settings: retention, the embedded engine's CPU and memory,
log search limits, the ingest queue and disk protection. The console shows
them under **Administration → Settings → Analytics**. `forge.toml` gives the
defaults ([Configuration: `[analytics]`](/docs/reference/configuration/#analytics));
overrides are stored in the operational database under the settings key
`analytics.overrides`. See
[Configure analytics retention and limits](/docs/guides/analytics-settings/).

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/analytics/settings` | `view` on the platform → settings view below |
| PUT | `/analytics/settings` | `analytics.configure` on the platform, MFA-verified session — `{settings, confirm?}` → apply report below. `409` `confirmation_required` when a confirmation is missing. Audited as `analytics.settings.update` with every changed setting (`old`, `new`, `override_value`) and the report; a value out of bounds is `400` and audited as denied |
| POST | `/analytics/restart` | `analytics.configure` on the platform, MFA-verified session — `{at?}` restarts the embedded engine now (no body, or `at` omitted or in the past) or at `at` (epoch ms, at most 7 days ahead) → `{at}`. A new schedule replaces the previous one. `400` for an external engine or a time more than 7 days ahead. Audited as `analytics.restart` |
| DELETE | `/analytics/restart` | `analytics.configure` on the platform, MFA-verified session — cancels the scheduled restart → `204`; `404` when none is scheduled. Audited as `analytics.restart.cancel` |

Without an MFA-verified session the three changing endpoints answer `403`
`step_up_required`. The generic `PUT /settings/{key}` and
`DELETE /settings/{key}` refuse keys that start with `analytics.` with `400`.

### Settings

Every setting is a number. The same name is used in the request, the
response, the audit log and the console.

| Setting | Group | Unit | Bounds | `forge.toml` key |
| --- | --- | --- | --- | --- |
| `node_metrics_days` | `retention` | days | 1–3650 | `retention.node_metrics_days`. Also `container_metrics_1m` |
| `top_processes_days` | `retention` | days | 1–3650 | `retention.top_processes_days` |
| `node_rollup_days` | `retention` | days | 1–3650 | `retention.node_rollup_days`. Also `container_metrics_1h` |
| `app_logs_days` | `retention` | days | 1–3650 | `retention.app_logs_days` |
| `cpu_quota_vcpu` | `process` | vCPUs, decimal; rounded up to whole CPUs | `0` (none) or 0.1–256 | `cpu_quota_vcpu` |
| `memory_budget_mb` | `engine_memory` | MiB | 256–1048576 | `embedded_memory_mb` |
| `search_max_concurrent` | `forge` | searches | 1–256 | `search.max_concurrent` |
| `search_timeout_secs` | `forge` | seconds | 1–3600 | `search.timeout_secs` |
| `search_max_lookback_days` | `forge` | days | 0 (no limit)–3650 | `search.max_lookback_days` |
| `search_threads` | `forge` | threads | 0 (engine default)–256 | `search.threads` |
| `search_memory_mb` | `forge` | MiB | 0 (engine default)–1048576 | `search.memory_mb` |
| `ingest_max_queued_mb` | `forge` | MiB | 1–65536 | `ingest.max_queued_mb` |
| `ingest_max_batch_rows` | `forge` | rows | 100–1000000 | `ingest.max_batch_rows` |
| `ingest_flush_interval_ms` | `forge` | ms | 10–10000 | `ingest.flush_interval_ms` |
| `min_free_disk_mb` | `forge` | MiB | 0–10485760 | `min_free_disk_mb` |

How each group takes effect:

| Group | Applied by | Verified by |
| --- | --- | --- |
| `retention` | `ALTER TABLE … MODIFY TTL` on the analytics tables, for the embedded engine and an external ClickHouse (the user needs `ALTER` on the tables) | The TTLs read back from the engine |
| `process` | Pins every thread of the embedded engine to `ceil(cpu_quota_vcpu)` CPUs (`sched_setaffinity`) and rewrites its thread pool sizes in `config.xml` | The affinity read back from the kernel. The pools resize at the next engine restart, so `pending_restart` becomes `true` |
| `engine_memory` | Rewrites `max_server_memory_usage` in the embedded engine's `config.xml` | `system.server_settings` within about 15 seconds, else `pending_restart` |

The `process` and `engine_memory` settings apply to the embedded engine
only. With any other engine, a request that sets one of them is refused
with `409` and the problem title `managed_externally`, and nothing is
stored. Sending `null` for them (removing an override) is allowed.
| `forge` | Forge's own log search, ingest queue and disk check | Immediate |

### `PUT /analytics/settings`

| Field | Type | Meaning |
| --- | --- | --- |
| `settings` | object | Setting name → number sets an override; `null` removes it (back to `forge.toml`); an absent setting is left alone. Unknown names are `400`. At least one setting is required |
| `confirm.shorten_retention` | boolean, default `false` | Required when a retention setting gets shorter: data older than the new limit becomes eligible for deletion by background merges |

The overrides are stored before anything is applied, so a setting that
cannot be applied now is applied when the engine is next started or
reachable.

```sh
curl -sS -X PUT https://<forge-host>/api/v1/analytics/settings \
  -H "Authorization: Bearer $FORGE_TOKEN" -H 'Content-Type: application/json' \
  -d '{"settings":{"app_logs_days":14,"cpu_quota_vcpu":1.5,"search_threads":null},"confirm":{"shorten_retention":true}}'
```

The answer is an apply report, one result per changed setting:

| Field | Type | Meaning |
| --- | --- | --- |
| `at` | epoch ms | When the change was applied |
| `results[].setting` | string | Setting name |
| `results[].status` | string | `applied`: in effect, and the engine (or Forge) reports the new value. `pending`: stored, applied when the analytics database is reachable or analytics is turned on. `pending_restart`: stored, takes effect when the engine restarts. `failed`: stored but not applied; `message` says why |
| `results[].effective` | number or `null` | The value in effect, as read back, when known |
| `results[].message` | string or `null` | Why a setting is not `applied` |

```json
{"at":1763700000000,"results":[
 {"setting":"app_logs_days","status":"applied","effective":14,"message":null},
 {"setting":"cpu_quota_vcpu","status":"applied","effective":2,"message":"the engine runs on 2 CPUs; restart it to resize its thread pools to match"},
 {"setting":"search_threads","status":"applied","effective":0,"message":null}]}
```

A missing confirmation is `409` with a problem document and the
confirmations to send. Nothing is stored. Send the request again with each
listed `confirm` field set to `true`:

```json
{"type":"https://docs.orbyn.dev/errors/confirmation_required","title":"confirmation_required","status":409,
 "detail":"Shortening retention (app_logs_days 30 → 14 days) makes existing data older than the new limit eligible for deletion by background merges.",
 "confirm":[{"confirm":"shorten_retention","message":"Shortening retention (app_logs_days 30 → 14 days) makes existing data older than the new limit eligible for deletion by background merges."}]}
```

### `GET /analytics/settings`

| Field | Type | Meaning |
| --- | --- | --- |
| `engine` | string | `off`, `embedded`, `clickhouse`, `duckdb`, `postgres` or `memory` |
| `managed` | boolean | The engine is run by Forge (`embedded`) |
| `ready`, `last_error` | boolean, string or `null` | As in `GET /analytics/status` |
| `settings[]` | array | One entry per setting: `setting`, `group`, `configured` (from `forge.toml`), `override_value` (number or `null`), `effective`, `min`, `max`, `applies` (`false`: a CPU or memory setting of an engine Forge does not run; changes are refused) |
| `usage.cpu_vcpu` | number or `null` | Embedded engine: vCPUs used over the last 10-second sample, from `/proc/<pid>/stat` |
| `usage.process_memory_bytes` | integer or `null` | Embedded engine: resident memory of its process (`VmRSS` from `/proc/<pid>/status`) |
| `usage.engine_memory_bytes` | integer or `null` | The engine's own tracked memory |
| `usage.server_memory_limit_bytes` | integer or `null` | The engine's memory budget (`0`: derived from host memory) |
| `usage.per_query_memory_bytes` | integer or `null` | Memory one search may use (`null`: engine default) |
| `usage.allowed_cpus` | integer or `null` | Embedded engine: CPUs its process may run on, read back from the kernel (`null`: not running or unknown) |
| `usage.free_disk_bytes` | integer or `null` | Free disk of the engine |
| `queue` | object | The ingest queue now: `queued_bytes`, `queued_requests`, `oldest_ms` (age of the oldest waiting batch, `null` when empty) |
| `process_limits_unavailable` | string or `null` | Why the CPU cap is not in effect (`null`: it is, none is set, or the engine is not embedded) |
| `pending_restart` | boolean | A memory budget or CPU cap change waits for an engine restart |
| `restart_at` | epoch ms or `null` | A scheduled engine restart |
| `last_apply` | object or `null` | The apply report of the last change since Forge started |
| `worker` | object | The analytics worker process; see below |
| `jobs[]` | array | Scheduled store jobs (`duckdb` only; empty for other engines): `name` (`rollups`, `retention` or `compaction`), `last_run` (epoch ms), `duration_ms`, `error` (string or `null`), `done` (rows folded, files dropped or files checkpointed) |

With `duckdb`, `applies` is `true` for `cpu_quota_vcpu` and
`memory_budget_mb`: they set DuckDB's threads and memory limit at once.

While the worker is down, `usage` and `queue` are empty, `ready` is
`false`, and `last_error` says why.

#### The `worker` object

The `forge-analytics-worker` process that `forge-server` supervises. Also
returned by `GET /analytics/status`.

| Field | Type | Meaning |
| --- | --- | --- |
| `status` | string | `off` (analytics is off; no worker), `starting`, `running`, `restarting` (waiting out the back-off), `incompatible` (a worker binary of another protocol version; not restarted until `forge-server` restarts) or `stopped` |
| `pid` | integer or `null` | Process id of the running worker |
| `rss_bytes` | integer or `null` | Resident memory at the last check (every 5 seconds; Linux) |
| `memory_limit_bytes` | integer or `null` | `[analytics] worker_memory_limit_mb` in bytes; `null` when there is no limit |
| `restarts` | integer | Restarts since `forge-server` started |
| `started_at` | epoch ms or `null` | When the running worker started |
| `last_restart_at` | epoch ms or `null` | When it was last restarted |
| `last_restart_reason` | string or `null` | `exited`, `unresponsive` (three health checks unanswered, or its agent listener certificate could not be renewed with less than a day left), `memory_limit` or `start_failed` |
| `last_error` | string or `null` | Why the last start failed, or why the worker is incompatible |

Each restart is audited as `analytics.worker.restart` (actor `system:forge`)
with `details.reason` and `details.detail`.

## Clusters and onboarding

| Method | Path | Permission |
| --- | --- | --- |
| GET / POST | `/clusters` | `view` / `workspace.manage` |
| GET / PUT / DELETE | `/clusters/{id}` | `view` / `workspace.manage` |

**Cluster body** (`POST /clusters`, `PUT /clusters/{id}`):

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `name` | string | required | Letters, digits, spaces, `-`, `_`, `.`; unique per workspace |
| `description` | string | `""` | Up to 500 characters |
| `environment_id` | string or `null` | `null` | Environment this cluster serves |
| `abbr` | string | initials of the name | Up to 4 letters or digits |
| `icon` | string | none | Data URL (PNG, JPEG, WebP, GIF or SVG), up to 128 KB; `""` removes it |
| `mode` | `default` or `swarm` | `default` | Set on `POST` only. On `PUT` it may be omitted or repeated; a different value returns `400`. See [Cluster modes](/docs/concepts/clusters/#cluster-modes) |

### Docker images, volumes and networks

On any Linux node with Docker Engine. The node's root helper runs each
operation; nothing needs the Docker TLS endpoint.

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/servers/{id}/docker/images` | `server.manage` on the node: `[{id, full_id, tags, digests, size, created, containers, dangling}]` |
| GET | `/servers/{id}/docker/volumes` | `server.manage`: `[{name, driver, mountpoint, scope, created, labels, managed}]` |
| GET | `/servers/{id}/docker/networks` | `server.manage`: `[{id, name, driver, scope, internal, attachable, subnets, containers, protected}]` |
| GET | `/servers/{id}/docker/disk-usage` | `server.manage`: `{images, containers, volumes, build_cache}`, each `{count, bytes}` |
| GET | `/servers/{id}/docker/events?since_secs=` | `server.manage`: up to 1000 recent events, newest first (`since_secs` at most 86400, default 300) |
| POST | `/servers/{id}/docker/ops` | By operation (below). `200` with the node's answer, or `202` with a `docker.op` job for pulls, pushes and prunes |

**Operations** (`{"op": "<name>", …}`):

| `op` | Fields | Permission |
| --- | --- | --- |
| `image_inspect`, `image_history` | `id` | `server.manage` |
| `image_remove` | `id`, `force` | `docker.image.manage` |
| `image_tag` | `id`, `repo`, `tag` | `docker.image.manage` |
| `image_pull` | `image` | `docker.image.manage` (job) |
| `image_push` | `image` (a registry with `allow_push`) | `registry.manage`, MFA (job) |
| `image_prune` | `all` (default: dangling only) | `docker.image.manage` (job) |
| `volume_inspect` | `name` | `server.manage` |
| `volume_create` | `name`, `labels` (local driver, no options) | `docker.volume.manage` |
| `volume_remove` | `name`, `force` | `docker.volume.manage` |
| `volume_prune` | | `docker.volume.manage` (job) |
| `network_inspect` | `id` | `server.manage` |
| `network_create` | `name`, `driver` (`bridge`/`overlay`), `subnet`, `gateway`, `internal`, `attachable` (overlay only), `labels` | `docker.network.manage` |
| `network_remove` | `id` | `docker.network.manage` |
| `network_connect`, `network_disconnect` | `network`, `container` (`force` for disconnect) | `docker.network.manage` |
| `network_prune` | (Forge's networks are kept) | `docker.network.manage` (job) |

`400` for registry credentials (`registry_auth`), driver options, reserved
labels (`forge.*`) and Docker's or Forge's own networks (`bridge`, `host`,
`none`, `ingress`, `docker_gwbridge`, `forge-*`).

### Container registries

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/registries` | `view` on the workspace: `[{id, workspace_id, name, kind, host, username, created_by, created_at, updated_at}]` (never the secret) |
| POST | `/registries` | `registry.manage` (MFA): `{name, kind, host, username, secret, allow_push}` → `201` the registry |
| PUT | `/registries/{id}` | `registry.manage` (MFA): the same body; without `secret` the stored one stays, unless `host`, `kind` or `username` changes (`422`) |
| DELETE | `/registries/{id}` | `registry.manage` (MFA) → `204` |

`kind` is `docker_hub`, `ghcr`, `gitlab`, `ecr` or `custom`. `host` is
`host[:port]` (a scheme or trailing slash is removed); Docker Hub's is
`docker.io`. `409` when the workspace already has a registry for the host.
Image pushes (`image_push`) need `registry.manage` and a registry for the
image's host with `allow_push` (default `false`).

### Deploy webhooks

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/webhooks` | `view` on the workspace (never the token or secret) |
| POST | `/webhooks` | `webhook.manage` (MFA) and `deploy` on the service (or `stack.manage` on the stack's environment): `{name, target, signature, tag_pattern, enabled}` → `201 {webhook, token, secret}` (shown once) |
| PUT | `/webhooks/{id}` | The same: changes `name`, `tag_pattern`, `enabled`; the target and signature stay |
| POST | `/webhooks/{id}/rotate` | The same: new token and secret → `{webhook, token, secret}` |
| DELETE | `/webhooks/{id}` | The same → `204` |
| GET | `/webhooks/{id}/deliveries` | `view`: the newest 100 deliveries `{at, outcome, reason, job_id, tag, source_ip}` |
| POST | `/hooks/{token}` | The secret URL (no session); body at most 64 KiB → `202 {jobs, status_url}` |
| GET | `/hooks/{token}/jobs/{job}` | The secret URL: `{id, kind, status, error}` of a job this webhook started |

`target` is `{"kind": "service", "id"}` or `{"kind": "stack", "id"}`.
`signature` is `none`, `github` (`X-Hub-Signature-256: sha256=<hmac(body)>`),
`gitlab` (`X-Gitlab-Token: <secret>` and a required, unique
`X-Gitlab-Event-UUID`) or `forge` (`X-Forge-Timestamp:
<unix>`, `X-Forge-Signature: sha256=<hmac("<timestamp>.<body>")>`, within 300
seconds). A delivery's tag comes from a JSON body's `"tag"`, only when
`tag_pattern` is set (`*` matches any characters); `?tag=` is accepted only
by an unsigned (`none`) webhook, since no signature covers the query. Each
signed delivery is accepted once: its marker (the `github` or `forge`
signature, or the GitLab event id) is kept in the database for 30 days. If
the deploy fails after a tag change, the previous image is restored.
Answers: `400` GitLab delivery without `X-Gitlab-Event-UUID`, `401` bad
signature, `403` disabled or the creator can no longer deploy, `404` unknown
URL, `409` replayed delivery, `422` refused tag (including `?tag=` on a
signed webhook), `429` rate limit (10 per minute per webhook, 60 per
source address), `503` the replay marker could not be recorded.

### Compose app templates

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/stack-templates` | `view` on the workspace: `[{id, title, description, category, compose, variables: [{name, label, default, secret}]}]` |
| POST | `/stacks/from-template` | As `POST /stacks`: `{template, environment_id, server_id, name, values}` → `201 {stack, preview}`. Empty secret variables are generated; unknown variables and values with a newline or `'` give `400` |

### Docker Swarm

Swarm clusters only (`mode: swarm`). Joins and leaves start a `swarm.member`
job; node changes and token rotation run through a manager at once. Join
tokens never leave the server.

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/clusters/{id}/swarm` | `view` on the cluster: `{cluster_id, mode, swarm, members, nodes, nodes_error, quorum}` |
| POST | `/clusters/{id}/swarm/members` | `swarm.manage` (MFA) on the cluster: join a node → `202 {job, installs}` |
| PATCH | `/clusters/{id}/swarm/members/{server}` | `swarm.manage` (MFA): change role, availability or labels → the swarm node |
| DELETE | `/clusters/{id}/swarm/members/{server}?force=` | `swarm.manage` (MFA): leave → `202` the job |
| POST | `/clusters/{id}/swarm/tokens/rotate` | `swarm.manage` (MFA): `{worker, manager}` → `204` |

**Join body:**

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `server_id` | string | required | A Linux node already assigned to the cluster |
| `role` | `manager` or `worker` | required | The first node must be `manager` |
| `advertise_addr` | string | Docker's choice | IP address or interface name other nodes reach this node on |
| `default_addr_pool` | string array | Docker's default | Overlay address pools (CIDR), first node only, at most 8 |
| `autolock` | boolean | `false` | Lock manager keys at rest, first node only |

`installs` lists Docker Engine install jobs Forge started first on a node
without a running engine; the join waits for them.

**Node body** (`PATCH`): `role` (`manager`/`worker`), `availability`
(`active`, `pause`, `drain`), `labels` (object; replaces all labels). At least
one is required. Demoting the last manager returns `409`.

**Errors:** `422` when the cluster is not a Swarm cluster, the node is not in
the cluster or not Linux, or the first node is not a manager; `409` while the
node is already joining, joined or leaving, when the last manager would leave
while workers remain, or when no manager answers.

| PATCH | `/servers/{id}` | `server.manage` on the node; with `cluster_id` also `server.enroll` on that cluster; leaving a cluster needs `server.enroll` on the cluster left; changing the environment needs `server.enroll` on the new and on the old environment — `{cluster_id, environment_id}`, both optional (`null` or omitted clears). With `swarm_role` (`manager`/`worker`) and a Swarm `cluster_id`, the node also joins the cluster's swarm (`swarm.manage`, MFA; see [Docker Swarm](#docker-swarm)). A node in a swarm cannot move to another cluster until it leaves (`409`). See [Moving a node between clusters](#moving-a-node-between-clusters) → the updated server |
| POST | `/nodes/probe` | `node.onboard` (MFA) — returns SSH host key |
| POST | `/nodes/onboard` | `node.onboard` (MFA) — starts `node.bootstrap` |
| GET | `/onboarding` | `server.enroll` — what the manual path needs: `{agents_available, install_url, agent_url, linux, windows_available, windows}`. `agents_available` is `true` when `agent_dist_dir` holds any agent build; `windows_available` when it holds `forge-agent-windows-x86_64.exe`, which releases do not include, so it is `false` unless you add your own build. `install_url` is `<forge-url>/install.sh`; `agent_url` is `agent_public_url`. `linux` and `windows` are command templates with a literal `<token>` placeholder |
| GET | `/onboarding/nodes` | Nodes being onboarded in the last 24 hours. Assisted installs need `view` on their `node.bootstrap` job; manual installs need `server.enroll` on the workspace. See [Onboarding console](#onboarding-console) |
| GET | `/onboarding/console?job_id=` | `view` on the job — the console of an assisted (SSH) install |
| GET | `/onboarding/console?token_id=` | `server.enroll` on the workspace — the console of a manual install, by enrollment token id |
| POST | `/enroll/progress` | Enrollment token (`Authorization: Bearer <enrollment-token>`), no session — the installer reports its output. See [Installer progress](#installer-progress) |
| GET | `/secrets` | `workspace.manage` — metadata only |
| DELETE | `/secrets/{id}` | `workspace.manage` |
| GET | `/install.sh?hostname=&terminal_account=&terminal_account_sudo=` | public — manual installer (no secrets). `terminal_account` names the terminal account the script creates; `terminal_account_sudo=1` or `true` gives it passwordless `sudo`. `400` when `terminal_account` breaks the [naming rules](#terminal-accounts). See [Node installer](/docs/reference/cli/node-installer/#the-endpoint) |
| GET | `/install.ps1` | public — Windows node installer (no secrets, no query parameters). The script carries the control plane URL, the agent URL and the SHA-256 of the Windows agent; it reads the token from `$env:FORGE_ENROLL_TOKEN`. `text/plain; charset=utf-8`. See [Node installer: Windows](/docs/reference/cli/node-installer/#windows-installps1) |
| GET | `/agent/download/{os}/{arch}/{binary}` | public — agent binaries, with an `x-checksum: sha256=<hex>` header. Linux: `os` `linux`, `arch` `x86_64` or `aarch64`, `binary` `forge-agent` or `forge-agent-helper`. Windows: only `/agent/download/windows/x86_64/forge-agent`, which serves `forge-agent-windows-x86_64.exe` (there is no helper on Windows). Releases do not include that file. `404` for anything else or a build missing from `agent_dist_dir` |

The `/install.sh` and `/install.ps1` responses carry `Cache-Control: no-store` and `Vary: Host, X-Forwarded-Host, X-Forwarded-Proto`: each embeds the address it was fetched from.

**Onboard body** (`POST /nodes/onboard`):

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `host` | string | required | Host name or IP the control plane reaches over SSH |
| `port` | number | `22` | SSH port |
| `username` | string | required | SSH login of a sudo-capable user. Saved as a terminal login for the node once it is online |
| `host_key_fingerprint` | string | required | The `SHA256:…` fingerprint from `POST /nodes/probe` that you confirmed (`400` otherwise) |
| `auth` | object | required | `{"type": "password", "password": "<password>"}` or `{"type": "private_key", "pem": "<openssh-private-key>", "passphrase": "<passphrase>"}` (`passphrase` optional) |
| `sudo_password` | string | none | When it differs from the login password, or with a key and no passwordless `sudo` |
| `cluster_id` | string | none | Cluster to join |
| `environment_id` | string | none | Environment to register in |
| `keep_credential` | boolean | `false` | Keep the credential in the vault after onboarding |
| `hostname` | string | none | Hostname to enroll under (first 63 characters) |
| `terminal_account` | string | none | Terminal account the installer creates and Forge saves. Same rules as `name` under [Terminal accounts](#terminal-accounts) (`400` otherwise). Empty or missing creates none |
| `terminal_account_sudo` | boolean | `false` | Passwordless `sudo` for `terminal_account`. Ignored without it |

### Moving a node between clusters

`PATCH /servers/{id}` with a different `cluster_id` hands the node over:

1. Forge checks the node's roles in the cluster it leaves. It answers `409`
   while the node is still an ingress edge of that cluster, for example:
   `the node is still the cluster's ingress edge (Cluster → Ingress); move
   that role to another node first`. Nothing changes.
2. Forge saves the membership and plans the node's address on the new
   cluster network. If that fails, the old membership is restored and the
   error is returned.
3. The old cluster's address plan drops the node. When the old cluster has a
   network, the node removes that WireGuard interface in the background,
   acting as the caller. A failure is logged; the request still succeeds.
4. When the new cluster has a network, its reconciler adds the node in the
   background.

Errors: `400` cluster or environment in another workspace, `403`, `404`, `409`
as above. Audited as `server.assign`.

### Onboarding console

The console shows what a node install is doing, line by line. Assisted
installs write each command (`$ …`) and the installer's output; manual
installs show what the installer sent to `/enroll/progress`. Secrets are
masked before lines are stored. Each line is `{t, step, line}`: `t` in epoch
ms, `step` the job step (`connect`, `install`, …) or `installer`, `line` at
most 400 characters.

`GET /onboarding/console` answers:

| Query | Response |
| --- | --- |
| `job_id` | `{status, error, lines}`: the job's status and last error. Lines come from memory while the job runs, and from the job's own copy afterwards, which survives a restart |
| `token_id` | `{status, server_id, lines}`: `status` is `failed` when a line starts with `installer failed` or `error: `, `succeeded` when a line is `installer finished` or a node enrolled with the token, otherwise `running` |

Errors: `400` neither `job_id` nor `token_id`, `403`, `404` unknown job or
token.

`GET /onboarding/nodes` returns an array of entries:

| Field | Assisted (`kind: "assisted"`) | Manual (`kind: "manual"`) |
| --- | --- | --- |
| `job_id` / `token_id` | The `node.bootstrap` job | The enrollment token |
| `status` | Job status | `running`, `succeeded` or `failed`, as above |
| `error` | Job's last error | (absent) |
| `host` | SSH target host | (absent) |
| `hostname` | Requested hostname, else the SSH host | `hostname` from the token, or `null` |
| `cluster_id` | From the job | `cluster_id` from the token, or `null` |
| `server_id` | Once enrolled | Once enrolled |
| `created_at`, `updated_at` | Epoch ms | `updated_at`: time of the last line |
| `active` | Job queued or running | Not failed, not finished, not enrolled |

:::note
Manual install consoles live in memory only: up to 2000 lines per install
and 500 installs, kept 24 hours after their last line. They are lost when
`forge-server` restarts. Only manual installs that reported at least one
line are listed.
:::

### Installer progress

`POST /api/v1/enroll/progress` takes one report from a manual installer.
`install.sh` and `install.ps1` call it at each step; failures there are ignored. A line that
starts with `installer failed` or `error: ` marks the install as failed, and
the line `installer finished` marks it as done.

| Item | Value |
| --- | --- |
| Authentication | `Authorization: Bearer <enrollment-token>`. Accepted, used or not, until one hour after the token expires |
| Body | Plain text, at most 2 KiB. Split into lines; control characters and empty lines are dropped. The token itself is replaced with `********` |
| `204` | Stored |
| `401` | Missing, malformed, unknown or expired token |
| `413` | Body larger than 2 KiB |
| `429` | The install's console already holds 2000 lines |

This example reads the token from `FORGE_ENROLL_TOKEN` and passes the
header from a file, so the token does not appear on a command line:

```sh
umask 077
printf 'Authorization: Bearer %s\n' "$FORGE_ENROLL_TOKEN" > report-header
printf 'downloading agent' | curl -fsS -X POST -H @report-header \
  -H 'Content-Type: text/plain' --data-binary @- \
  "https://<forge-host>/api/v1/enroll/progress"
rm -f report-header
```

## App Store

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/catalog` | signed in — packages (no recipes) and roadmap |
| POST | `/packages/install` | `package.manage` (MFA) on every node — `{package, version, params, server_ids}`; one `package.action` job per node |
| GET | `/installations?server_id=` | `view` (filtered) |
| POST | `/installations/{id}/actions` | `{action}`: `start`, `stop`, `restart`, `uninstall`, `install` (repair) — `package.manage`; `status` — `server.manage` |
| POST | `/installations/{id}/update` | `{build}`: update to exactly this build, which must be the node's reported candidate — `package.manage` |
| PUT | `/installations/{id}/policy` | `{update_policy}`: `notify` or `auto_patch` — `package.manage` (MFA). Automatic patches run as the person who last set `auto_patch`, with their current roles |
| GET | `/package-updates` | Tracked installations with installed, candidate and running builds, update kind and vendor release line (`view`) |
| GET | `/traffic?cluster_id=` | Route throughput through the edges, last minute: total, per edge and endpoint, per backend node (`view` per node) |
| GET | `/package-releases` | Vendor release lines per product: latest, LTS, EOL (any signed-in user) |
| POST | `/package-updates/check` | Ask every node you manage for fresh build facts (`server.manage` per node) |

## Ingress, sites and DNS

| Method | Path | Permission |
| --- | --- | --- |
| GET / POST | `/sites` | `view` / `workspace.manage` — `{name, domains, upstreams?, standby, balance, tls, acme_email, force_https, websockets, max_body_mb, rate_limit, custom, edges, cluster_id?, targets?, standby_targets?, dns?}`; `upstreams` defaults to an empty list: with `targets` they are resolved from them, and a site that ends up with no upstream (none given, none resolved) is refused with `400`; with `cluster_id` the edges come from the cluster profile; `targets` are `{kind: app, service_id, port}` (every node the service runs on, at its cluster-network address; re-resolved on apply), `{kind: node, server_id, port}` or `{kind: external, address}`; `tls`: `off`, `acme` or `uploaded` (with `cert_id`, which must cover every domain) |
| GET / PUT / DELETE | `/sites/{id}` | `view` / `workspace.manage`; a non-empty `custom` also needs `server.command` (MFA) on every edge. Sites of another workspace return `404` |
| POST | `/sites/preview` | `view` on the workspace — the same body as `POST /sites`; resolves edges and targets exactly as save and apply do, stores nothing and contacts no node. → `{variant, config, http, https, custom, edges, upstreams, standby, targets: [{target, standby, upstreams: [{address, standby, server_id, state}]}]}`. `variant` is `https` with TLS and `http` when `tls` is `off`; `config` is that variant's rendered config (what the edges run); `https` is `null` when `tls` is `off`. `400` when the spec is invalid, a service or node target is not in this workspace, or a service is not placed yet |
| POST | `/sites/{id}/apply` | `view` on the workspace + `proxy.manage` (MFA) on every edge (and the DNS node); a non-empty `custom` also `server.command` (MFA) on every edge; `tls: uploaded` also `certificate.use` (MFA). `400` without edges |
| POST | `/sites/{id}/remove` | `view` on the workspace + `proxy.manage` (MFA) on every edge. `400` without edges |
| POST | `/sites/{id}/renew` | `view` on the workspace + `proxy.manage` (MFA) on every edge — force-renews the Let's Encrypt certificate; returns the certificate state per edge |
| GET / PUT | `/clusters/{id}/profile` | `view` / `workspace.manage` on the cluster — `{ingress: {mode, edges: [{server_id, role, public_ip?}], failover, vrrp?, nginx, dns?: {server_id, ttl, auto_failover}}}`; GET also returns `members` and `status` (last apply, `health` per edge, published `dns`) |
| POST | `/clusters/{id}/profile/apply` | `proxy.manage` (MFA) on every edge; a non-empty `nginx.custom` also `server.command` (MFA) on every edge (also needed to save it); with `dns`, `proxy.manage` on the PowerDNS node — returns `{edges, dns}`; `400` without edges |
| POST | `/clusters/{id}/profile/remove` | `proxy.manage` (MFA) on every edge; `400` without edges. → `{edges}` with a `status` per edge (`removed`, `failed`, `pending`) and its `job_id`. An edge that is connected gets up to 120 seconds to answer; an offline edge (or one whose agent holds no link to this server) answers `pending` after about a second, and its job runs when the edge reconnects. Edges reporting `removed` are dropped from the profile's edge list, so they no longer block a move out of the cluster; failed or pending edges keep the role. Mode, Nginx, VRRP and DNS settings are kept, so adding an edge again restores them; an empty edge list means no edge. Removal edits the stored profile without `workspace.manage`: `view` on the cluster plus `proxy.manage` (MFA) on every edge is enough, because only edges whose removal succeeded are cleared |
| GET | `/clusters/{id}/live` | `view`; per edge (where the caller has `server.manage`): Nginx, keepalived, VIPs held |
| GET | `/clusters/{id}/routing` | `view` — `{endpoints, profile}`. `profile`: `true` when the cluster has an ingress profile. Per endpoint: `id`, `name`, `domains`, `tls`, `edges`, `targets` (internal names and resolved upstreams), `source` and `linked`. `source` is `profile` for endpoints linked to this cluster (`cluster_id`), and `edges` for endpoints of the same workspace with no `cluster_id` whose explicit edges include a node of this cluster (`linked: false`) |
| GET / POST | `/certificates` | `view` / `workspace.manage` — POST `{name, chain_pem, key_pem}`; validated (key match, expiry, names); the key is sealed and never returned |
| DELETE | `/certificates/{id}` | `workspace.manage`; refused while an endpoint uses it |
| POST | `/servers/{id}/dns` | `{op: zones\|records\|create_zone\|delete_zone\|set_records\|delete_records, …}` — reads `server.manage`, changes `proxy.manage` (MFA) |

## Services

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/services?environment_id=&cluster_id=` | `view` (filtered: on the environment, or on the service itself for a grant at one service) — with instances and `can_deploy`; secret values never included. `cluster_id` keeps services in that cluster's environments and is ignored when `environment_id` is given |
| POST | `/services` | `deploy` on the environment — `{environment_id, name, runtime, command, working_dir, listen_port, image, ports, volumes, env_files, restart, env:[{key, value, secret}], dependencies:[{package, version, params}], cpu_millis, memory_mb, entrypoint, user, hostname, labels, extra_hosts, healthcheck, stop_timeout_secs, read_only, log_max_mb}`; only `environment_id`, `name` and `runtime` are required. `runtime` is `systemd` or `container` (`400` otherwise). Every field, with its type and default, is listed under [Service definition](/docs/reference/cli/services/#service-definition). `listen_port` (integer 1–65535, optional) is the TCP port a `systemd` service listens on; Forge's default health check connects to it. It is refused for containers (containers publish `ports`). Container `volumes` may not mount the Docker socket, system paths (`/`, `/etc`, `/var/lib/docker`, …) or Forge's folders (`/opt/forge`, `/var/lib/forge-agent`, `/etc/forge`); the node checks the resolved path again |
| GET / PUT / DELETE | `/services/{id}` | `GET`: `view` on the service (a grant on its environment covers it); includes `can_deploy`. `PUT` / `DELETE`: `deploy` on the environment (PUT: empty secret value keeps the stored one) |
| PUT | `/services/{id}/instances` | `deploy` + `server.manage` on each node — `[{server_id, role: active\|standby}]`. For a `systemd` service, each newly added Linux node gets an `app.action` job with action `prepare`: the node creates the service's account and folder `/srv/forge/apps/<environment>/<service>`, and Forge approves a writable service file root on it, so you can upload code before the first deploy. Removing a placement that was never deployed removes that root |
| POST | `/services/{id}/deploy` | `deploy` on the service (a grant on its environment covers it) → `202` with the array of jobs started: first the `package.action` install jobs, then one `app.action` job per node. Installs: for a container, Docker Engine (package `docker`) on each Linux node that has no Docker Engine installation from the App Store (installed or being installed) and that answers that Docker is not available (the server asks the connected node, read-only, and waits up to 15 seconds; an engine installed outside Forge, or a node that does not answer, gets no install); then one per node and runtime of the definition's `dependencies` that the node lacks (not installed in that version, and not being installed). Starting an install needs `package.manage` (MFA) on that node; otherwise `403` and nothing starts. Each `app.action` job waits for its node's installs. `409` when the service is placed on no node. Audited as `service.deploy` with `nodes`, `privileged_mounts` and `runtime_installs` (the number of install jobs started). A container volume whose host path lies outside `/srv/forge/apps/<environment>/<service>/` and the node's approved file roots also needs `stack.privileged` (MFA); otherwise `403` and no job starts. The job payload lists such volumes in `privileged_mounts` |
| POST | `/services/{id}/instances/{server_id}/actions` | `{action, tail?, cursor?}`: `prepare`, `deploy`, `start`, `stop`, `restart`, `remove` (`deploy`); `status`, `logs`, `follow` (`view`), checked on the service, so a grant at one service is enough. Another action answers `400`. Each action runs as an `app.action` job on the service, which also needs `job.create` there. `tail` is clamped to 0–5000. `follow` returns one segment of new lines (about 50 s) and the `cursor` to pass to the next call. `deploy` has the same volume rule as `/services/{id}/deploy` and installs nothing |

`can_deploy` (boolean) tells the console whether the caller holds `deploy`
on the service (directly or through its environment), so it can disable actions it knows would be
refused. It is a hint: every action is still authorized when it runs.

`prepare` (`systemd` services, Linux nodes) creates the service's account,
its folder and its file-explorer store, and starts nothing. It is
idempotent. A successful `prepare` or `deploy` also approves a writable
`service` file root on the service's folder (`RootSpec` `app`, with
`service_id` set) unless a root on that folder exists; `remove` and deleting
the service remove it. These roots are audited as `file.root.create` /
`file.root.delete` with `automatic: true`, and the account as
`service.instance.prepared` (a deploy as `service.instance.deployed`).

### Service health checks

Startup, readiness and liveness checks of one service (D-085). The
policy's fields are listed in [Health policy](/docs/reference/health-policy/).

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/services/health-summary?environment_id=` | `health.view` (filtered per service) — `{items: [{service_id, instances: [{server_id, status}]}], truncated}`. Covers at most 500 services; `truncated` is `true` when more were visible. `environment_id` (optional) limits it to one environment |
| GET | `/services/{id}/health` | `health.view` — `{policy, summaries, legacy_healthcheck, updated_at, can_configure, can_command, instances, warnings}` |
| POST | `/services/{id}/health/validate` | `health.view` — `{policy}` → `{valid, errors: [{field, message}], summaries, runs_code}`. Nothing is saved |
| PUT | `/services/{id}/health` | `health.configure`, plus `health.command` (MFA) when a check is `exec` or `shell`, with or without `show_output` — `{policy, expected_updated_at?}` → the same body as `GET`. Audited as `service.health.update`, a failed save included |

- `summaries` holds one `{purpose, name, enabled, summary}` per configured check, then the effective default checks with `default: true` and `name: "default"` (for example `systemd unit active and TCP connect to port 8080, every 10 s; unhealthy after 3 failures.`). `summary` is the plain-language description the console shows.
- `instances` has one entry per node the service is placed on: `{server_id, status, checks}`. `status` is `healthy`, `degraded`, `unhealthy` or `unknown` (see [Status per node](/docs/concepts/health-checks/#status-per-node)). Each check is `{purpose, name, state, reason, detail, stale, default, received_at}`; `state` reads `unknown` once stale, and `reason` is `node_offline` after the node went offline. Configured checks are listed even before the node reports them: `state` is `unknown` (or `disabled`) and `received_at` is `null`. Results of checks the policy no longer has are left out.
- `PUT` sends the saved policy to every node the service is placed on, with the signed app command `health`. A node that reports results under another policy revision gets the policy again, at most once every 10 minutes per node and service. A node that reports results for a policy with an `exec` or `shell` check, or with a probe of a remote host, gets it again every 10 minutes. See [Policy delivery](/docs/reference/health-policy/#policy-delivery).
- Each check's `detail` holds the output of a failed `exec` or `shell` run only when that check has `show_output: true`; otherwise it reads `exit code <n>`.
- `warnings` lists `{code, server_id, message}`. `ambiguous_listen_port`: another service on that node declares the same listening port, so the default TCP check is off there.
- `legacy_healthcheck` is the container's Docker `HEALTHCHECK`, if any. Docker runs it; Forge doesn't convert it.
- `can_configure` and `can_command` say whether you may save HTTP/TCP checks (`health.configure`) and exec/shell checks (`health.command`). With MFA required, `can_command` is `true` only in an MFA-verified session.
- `validate` reports unknown fields and wrong types as an error with an empty `field`, instead of a `400`.
- `errors[].field` is a path into the policy, for example `readiness[0].probe.port`.
- `PUT` answers `422` with every problem in the message when the policy is invalid, and `409` when `expected_updated_at` doesn't match the stored `updated_at` (someone changed the service since you read it). An empty policy removes all checks.
- Besides the policy rules, a probe may not target another node of the fleet (its host name, its `.local` name or its fabric address).
- A `vault:<id>` header secret needs the right to reveal that item (platform administrator, `secret.read` with MFA, not with an API key): `403` otherwise, `404` when the item doesn't exist.
- A probe with a `vault:<id>` header and a `host` other than the workload must have `scheme: https` and `tls_verify: true`. Otherwise the policy is invalid, with the error on `probe.scheme` or `probe.tls_verify`: `a vault secret is sent to another host only over HTTPS with certificate verification`.
- `PUT` records who saved the policy together with the revision saved, in the same transaction as the policy. A node gets a vault value only while the stored policy's revision is the one that person saved. See [Vault header values](/docs/reference/health-policy/#vault-header-values).
- A `PUT` that fails changes nothing and is audited as a failed `service.health.update`.
- The audit entry lists each check's purpose, name, type, target and the vault items it uses. It never contains scripts, header values or query strings.

```sh
curl -s -X POST -H "Authorization: Bearer <api-key>" -H "Content-Type: application/json" \
  "https://<forge-host>/api/v1/services/<service-id>/health/validate" \
  -d '{"policy": {"readiness": [{"name": "api", "probe": {"type": "http", "port": 8080, "path": "/ready"}}]}}'
```

```json
{
  "valid": true,
  "errors": [],
  "summaries": [
    {
      "purpose": "readiness",
      "name": "api",
      "enabled": true,
      "summary": "Check GET http://the workload:8080/ready every 10 s (timeout 2 s). Mark unready after 3 consecutive failures; ready again after 1 success."
    }
  ],
  "runs_code": false
}
```

## Releases and deployments

Releases and rollouts of a service from a `forge.deploy.yaml` document. The
document is described in [Deployment YAML](/docs/reference/deployment-yaml/);
the model in [Deployments and releases](/docs/concepts/deployments/). `{id}`
under `/services` is the service's id.

| Method | Path | Permission |
| --- | --- | --- |
| POST | `/deployments/validate` | Signed in — `{yaml, environment?}` → `{valid, service, environment, runtime, artifact, policy}`. Checks the schema, artifact and policy only; changes nothing. `400` with a dotted path on an invalid document |
| POST | `/deployments/plan` | As `POST /deployments` — `{yaml, environment?}` → the [plan view](#plan-view). Starts nothing |
| POST | `/deployments` | `deploy` on the service (on its environment when the document creates it); `server.manage` on each node the placement adds or removes; `stack.privileged` (MFA) on the environment for container volumes outside the app folder — `{yaml, environment?}` → `202` `{deployment, plan, installs}`. `installs` are the runtime install jobs started first (Docker Engine, App Store runtimes), as for `POST /services/{id}/deploy`. `409` when the service has a deployment running. Audited as `deployment.apply` |
| GET | `/services/{id}/releases?limit=` | `view` on the service — releases, newest first. `limit` 1–200, default 200 |
| GET | `/services/{id}/releases/{number}` | `view` — one release. `404` when there is no such number |
| GET | `/services/{id}/deployments?limit=` | `view` — deployments, newest first. `limit` 1–200, default 200 |
| POST | `/services/{id}/deployments` | `deploy` on the service, plus the node and volume rules of `POST /deployments` — `{release}` (required) → `202` `{deployment, plan, installs: []}`. Rolls an existing release out again on the service's current nodes with its own policy. Audited as `deployment.redeploy` |
| POST | `/services/{id}/rollback` | `deploy` — `{release?}` → `202` `{deployment, plan, installs: []}`. Without `release`, the newest release of a succeeded deployment that is not the current one. `422` when there is none, or `release` is the current one. Audited as `deployment.rollback` |
| GET | `/services/{id}/slots` | `view` — `{current_release, slots}`. `current_release` is `0` before the first release |
| GET | `/deployments/{id}` | `view` on the service — the deployment with `service`, `steps` (one line per plan step), `events` (up to 1000), `can_control` and `can_approve` |
| GET | `/deployments/{id}/events?after=` | `view` — events with `seq` greater than `after` (default 0), up to 1000 |
| POST | `/deployments/{id}/pause` · `/resume` · `/abort` | `deploy` on the service — `{}` → the deployment. Audited as `deployment.request` |
| POST | `/deployments/{id}/approve` · `/reject` | `deploy.approve` on the service — `{}` → the deployment. `403` for the deployment's author; `409` when it is not `awaiting_approval`. Audited as `deployment.request` |

All control requests answer `409` when the deployment has finished, and
`404` for an unknown action name. The request is recorded on the
deployment and acted on by the rollout job between steps; the response
shows the deployment right after recording it.

The rollout job (`deploy.rollout`) also audits `deployment.succeeded`,
`deployment.failed` and `deployment.aborted`.

### Deployment object

| Field | Type | Description |
| --- | --- | --- |
| `id` | string | Deployment id |
| `service_id`, `workspace_id` | string | The service and its workspace |
| `release_id`, `release_number` | string, integer | The release rolled out |
| `from_release` | integer | The release that served when it started (`0`: none) |
| `kind` | `apply`, `rollback`, `auto_rollback` | Why it runs |
| `strategy` | string | `rolling`, `recreate`, `blue-green`, `canary`, `manual` |
| `state` | string | `pending`, `running`, `paused`, `awaiting_approval`, `succeeded`, `failed`, `aborted`, `rolled_back` |
| `request` | string or null | A request not yet acted on: `pause`, `resume`, `abort`, `approve`, `reject` |
| `job_id` | string or null | The `deploy.rollout` job |
| `plan` | object | The planner's plan: `{strategy, slotted, steps, downtime, notes, targets}`. Each step has `op`: `start`, `gate`, `route`, `wait`, `approve` or `stop` |
| `step` | integer | Steps done |
| `error` | string or null | Why it failed |
| `created_by` | string | Principal that started it |
| `created_at`, `updated_at`, `finished_at` | epoch ms | `finished_at` is null while it runs |

Events: `{deployment_id, seq, at, level, server_id, message}`; `level` is
`info`, `warn` or `error`.

### Release object

| Field | Type | Description |
| --- | --- | --- |
| `id`, `service_id`, `workspace_id` | string | Ids |
| `number` | integer | Per service, from 1 |
| `spec` | object | The service definition the release carries |
| `env` | list | `{key, value, secret}`; secret values are never returned |
| `artifact` | object | `{kind: "image", image, digest}`, `{kind: "git", repo, commit, path}` or `{kind: "archive", url, sha256, strip_components}`. A tag-only image gets its `digest` when the first node pulls it |
| `policy` | object | The resolved rollout policy |
| `source`, `source_sha256` | string or null | The YAML text and its SHA-256 (null for a release not made from YAML) |
| `description` | string | `metadata.description` |
| `created_by`, `created_at` | string, epoch ms | Who made it, and when |

### Slot object

| Field | Type | Description |
| --- | --- | --- |
| `service_id`, `server_id` | string | The service and node |
| `slot` | string | `""` (unslotted instance), `a` or `b` |
| `release_number` | integer | The release that runs in it |
| `weight` | integer | Traffic weight 0–100 (0: out of traffic) |
| `state` | string | `starting`, `started`, `ready` or `failed` |
| `ports` | object | Service port → the copy's own port, for example `{"8080": 20004}` |
| `updated_at` | epoch ms | Last change |

### Plan view

| Field | Type | Description |
| --- | --- | --- |
| `service`, `environment` | string | Names |
| `creates_service` | boolean | The document creates the service |
| `release` | integer | The number the release gets (or the one redeployed) |
| `artifact` | string | One-line artifact label |
| `strategy` | string | Strategy name |
| `slotted` | boolean | The release runs next to the old one in another slot |
| `downtime` | boolean | Requests fail while the plan runs |
| `nodes`, `added_nodes`, `removed_nodes` | list of strings | Node names |
| `steps` | list of strings | Each step for people |
| `notes` | list of strings | What the plan does and why |
| `plan` | object | The raw plan, as on the deployment |

```sh
curl -s -X POST -H "Authorization: Bearer $FORGE_TOKEN" -H "Content-Type: application/json" \
  "https://<forge-host>/api/v1/deployments/plan" \
  --data "$(jq -Rs '{yaml: ., environment: "staging"}' forge.deploy.yaml)"
```

## Service storage

Buckets of a managed object store attached to a service. See
[Attach a bucket to a service](/docs/guides/attach-bucket-to-service/).

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/services/{id}/storage` | `view` on the service's environment → `{attachments: [Attachment], can_change}` |
| POST | `/services/{id}/storage` | `deploy` on the environment. Also `storage.manage` (MFA) on the store, unless the store belongs to that environment and no service of another environment has the bucket attached (`409 bucket_attached_elsewhere` otherwise). Body below → `201` with the same body as `GET`. Audited as `service.storage.attach` |
| DELETE | `/services/{id}/storage/{access_key}` | `deploy` on the environment, whoever owns the store → `200` with the same body as `GET`. Audited as `service.storage.detach` |

**Attach body** (`POST /services/{id}/storage`):

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `store_id` | string | required | A managed RustFS store. Other stores answer `409` |
| `bucket` | string | required | 3–63 lowercase letters, digits, dots and hyphens. Must exist (`404`) unless `create_bucket` is set |
| `create_bucket` | bool | `false` | Create the bucket when it doesn't exist |
| `access` | string | `readwrite` | `read` or `readwrite` |
| `prefix` | string | `S3` | Prefix of the variables: an uppercase letter, then uppercase letters, digits and underscores, at most 24 characters, not ending in `_` (`400`) |

**Attachment object:**

| Field | Type | Notes |
| --- | --- | --- |
| `access_key` | string | The key Forge issued for this attachment; also its id in the `DELETE` path |
| `store_id` | string | |
| `store_name` | string \| null | `null` when the store no longer exists |
| `endpoint` | string \| null | The S3 endpoint the service is given |
| `bucket` | string | |
| `access` | string | `read` or `readwrite` |
| `prefix` | string | |
| `env` | object | The names of the variables: `{endpoint, region, bucket, access_key, secret_key}`, for example `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`. Never values |
| `vault_item_id` | string | The vault item that holds the secret |

- `can_change` says whether you have `deploy` on the environment. That is enough for a store whose `environment_id` is the service's environment. For a shared store or a store of another environment, `storage.manage` (MFA) is checked on the store when you attach or detach. See [Ownership](/docs/concepts/object-storage/#ownership).
- Attaching answers `409` when another attachment of the service uses the prefix, or when the service's own environment already defines one of the five variables.
- The service gets the variables with its next deploy or other node command. Nothing is written to the service's own environment; a variable the service defines itself wins.
- Detaching deletes the key at the store and its vault item at once. When the store no longer exists, only the vault item is removed, and `storage.manage` is not needed.
- The secret is never returned. Reveal it with `POST /vault/items/{id}/reveal`.

```sh
curl -s -X POST -H "Authorization: Bearer <api-key>" -H "Content-Type: service/json" \
  "https://<forge-host>/api/v1/services/<service-id>/storage" \
  -d '{"store_id": "<store-id>", "bucket": "media", "create_bucket": true, "prefix": "MEDIA"}'
```

```json
{
  "attachments": [
    {
      "access_key": "FAK3X9Q2M7T1ZB6W4KHD",
      "store_id": "0193a1e2-7c1b-7d42-9b0e-5f2a6c1d8e90",
      "bucket": "media",
      "access": "readwrite",
      "prefix": "MEDIA",
      "vault_item_id": "0193a1e2-9f04-7a11-8c55-2b7d4e6f1a23",
      "env": {
        "endpoint": "MEDIA_ENDPOINT",
        "region": "MEDIA_REGION",
        "bucket": "MEDIA_BUCKET",
        "access_key": "MEDIA_ACCESS_KEY_ID",
        "secret_key": "MEDIA_SECRET_ACCESS_KEY"
      },
      "store_name": "Media store",
      "endpoint": "http://<node-address>:9000"
    }
  ],
  "can_change": true
}
```

## Object storage

Managed stores are authorized on their first member node, connected
(external) stores on the workspace. A store that belongs to an environment
(`environment_id` is set) also accepts `view` and `storage.data` from callers
who hold that action on the owning environment. `storage.manage` is never
accepted from the environment. A caller who reaches a store only through its
environment gets the limited store object and a filtered key list (below). A
refused request on an owned store is audited with `store_id` and
`environment_id` in the event details. S3 calls run from `forge-server` to the store
endpoint. See [Object storage](/docs/concepts/object-storage/) and
[Ownership](/docs/concepts/object-storage/#ownership).

A caller without such a role who holds a live [bucket grant](#bucket-grants)
reaches the four object endpoints of the granted bucket, sees the store as
the limited store object with `by_grant: true`, and sees only the granted
buckets in the bucket list. A grant opens no other endpoint in this section.

| Method | Path | Permission and behaviour |
| --- | --- | --- |
| GET | `/object-stores?environment_id=` | `view` (filtered) → `[Store]`; each element is the limited store object when you reach it only through its environment or only through a live bucket grant (`by_grant: true`). `environment_id` (string, optional): only the stores that environment owns; stores you reach only through a bucket grant are left out of that list |
| GET | `/object-stores/{id}` | `view`, or a live bucket grant on the store → `Store`, or the limited store object when you reach it only through its environment or only through a grant (`by_grant: true`) |
| PUT | `/object-stores/{id}/owner` | `storage.manage` (MFA) on the store, and `deploy` on the environment it is assigned to — assign the store to an environment or clear its owner. Body below → `200 Store`; `409 attached_elsewhere` without `force` when other environments' services use its buckets. Audited as `storage.owner.set` |
| POST | `/object-stores` | `storage.manage` (MFA) on the node — install a managed RustFS store (standalone or distributed). Body below → `202 {store, job, jobs}`. `storage.manage` is checked on every member node. With `environment_id`, also `deploy` on that environment |
| POST | `/object-stores/connect` | `storage.manage` (MFA) on the workspace — connect an external store. Body below → `201 Store`. The credential is checked first; on failure nothing is saved. With `environment_id`, also `deploy` on that environment |
| POST | `/object-stores/{id}/actions` | `{action, delete_data?, confirm?}`. Managed: `install` (repair), `start`, `stop`, `restart`, `disable` (stops; status `disabled`), `enable` (starts), `uninstall` (`storage.manage`), `status` (`server.manage`) → `202 {jobs: [Job], skipped?: [{server_id, result}]}`, one `storage.action` job per member; `409` while the store is busy. `uninstall` also works on `failed` and `error` stores; members on deleted or revoked nodes are dropped and listed in `skipped`. `delete_data` (bool, default `false`, `uninstall` only) deletes the data paths that carry this store's `.forge-objstore` marker (symlinks, overlaps with other stores and unmarked paths are skipped and logged); it needs `confirm` equal to the store name (`400` otherwise) and an MFA-verified session (`403 step_up_required`). External: `disconnect` only (`storage.manage`) → `200 {disconnected: <id>}`, deletes the store and its vault credential |
| POST | `/object-stores/{id}/test` | `storage.data` — runs `ListBuckets` → `{ok: true, latency_ms, buckets: <count>}` or `{ok: false, latency_ms, error}` |
| GET | `/object-stores/{id}/buckets` | `storage.data`, or a live bucket grant on the store → `[{name, created}]`. Through grants only: just the buckets you hold a live grant for |
| POST | `/object-stores/{id}/buckets` | `storage.data` — `{name}`: 3–63 of `a-z0-9.-`, starting and ending with a letter or digit → `201 {name}` |
| DELETE | `/object-stores/{id}/buckets/{bucket}` | `storage.manage` (MFA) → `204`; `409` when not empty (`BucketNotEmpty`) |
| GET | `/object-stores/{id}/buckets/{bucket}/objects?prefix=&token=&limit=` | `storage.data`, or a live bucket grant for this bucket (`read` or `readwrite`) — delimiter `/`, `limit` default `200`, `prefix` max 1024 bytes → `{objects: [{key, size, last_modified, etag}], prefixes: [string], next_token: string \| null}`. Not audited |
| GET | `/object-stores/{id}/buckets/{bucket}/object?key=` | `storage.data`, or a live bucket grant for this bucket (`read` or `readwrite`) — streamed download, `Content-Disposition: attachment`. Audited as `storage.object.download` |
| PUT | `/object-stores/{id}/buckets/{bucket}/object?key=` | `storage.data`, or a live `readwrite` bucket grant for this bucket — raw body, `Content-Length` required (`411`), max 5 GiB (`413`), `Content-Type` passed through → `201 {bucket, key, size}`. Audited as `storage.object.upload` |
| DELETE | `/object-stores/{id}/buckets/{bucket}/object?key=` | `storage.data`, or a live `readwrite` bucket grant for this bucket → `204`. Audited as `storage.object.delete` |
| POST | `/object-stores/adopt` | `storage.manage` (MFA) on the node — adopt the App Store `rustfs` container as a managed standalone store. Body below → `202 {store, job}`. With `environment_id`, also `deploy` on that environment |
| GET | `/object-stores/{id}/keys` | `storage.data` — managed RustFS stores only → `[Key]`, the service accounts of the store's root user. When you reach the store only through its environment: only the keys attached to services of that environment |
| POST | `/object-stores/{id}/keys` | `storage.manage` (MFA) — managed RustFS stores only. Issue an access key limited to one bucket. Body below → `201 {access_key, name, bucket, access, vault_item_id}`. The secret is sealed in the vault and never returned |
| POST | `/object-stores/{id}/keys/{access_key}/rotate` | `storage.manage` (MFA) — Forge-issued keys only. Creates a new key with the same bucket and access, tests it, updates the same vault item, then deletes the old key → `200 {access_key, bucket, access, vault_item_id}` with the new access key |
| DELETE | `/object-stores/{id}/keys/{access_key}` | `storage.manage` (MFA) — deletes the key at the store and its vault item, if Forge issued it → `204`. Not for the store's root key (`409`) |
| GET | `/object-stores/{id}/grants?all=&limit=` | `storage.manage` (MFA) — the store's [bucket grants](#bucket-grants) → `{grants: [Grant]}` |
| POST | `/object-stores/{id}/grants` | `storage.manage` (MFA) — grant one account access to one bucket. Body below → `201 Grant`. Audited as `storage.grant.create` |
| DELETE | `/object-stores/{id}/grants/{grant_id}` | `storage.manage` (MFA) — revoke a grant → `204`. Audited as `storage.grant.revoke` |
| GET | `/object-stores/grants/mine` | Signed in — your own live bucket grants, in a reduced form → `{grants: [{id, store_id, store, bucket, access, expires_at, status}]}` |

Object keys are 1 to 1024 bytes of UTF-8 with no control characters and no
`.` or `..` path segments (`400`). Downloads are sent with
`Content-Security-Policy: sandbox; default-src 'none'` and `nosniff`.

Downloads and uploads share a limit of 8 concurrent transfers per server;
beyond it they return `429 busy`. Audit actions: `storage.install`, `storage.connect`,
`storage.disconnect`, `storage.adopt`, `storage.owner.set`, `storage.start`, `storage.stop`, `storage.restart`, `storage.uninstall`,
`storage.bucket.create`, `storage.bucket.delete`, `storage.object.upload`, `storage.object.download`,
`storage.object.delete`, `storage.key.create`, `storage.key.rotate`, `storage.key.revoke`,
`storage.grant.create`, `storage.grant.revoke`.
Attaching a bucket to a service is audited on the service:
`service.storage.attach`, `service.storage.detach`.

The three `storage.object.*` events carry `store_id`, `bucket`, `key`, `size`
(not on delete) and `grant_id`: the bucket grant that allowed the request, or
`null` when a role did. A refused request on one of the four object endpoints
is audited as a denied `storage.data` on the node or workspace, with
`store_id`, `environment_id`, `bucket` and `access` (`read` or `readwrite`: what
the request needed) in the details.

**Install body** (`POST /object-stores`):

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `name` | string | required | Display name, unique |
| `slug` | string | required | Instance name `[a-z][a-z0-9-]{0,30}`, unique on each member node |
| `provider` | string | `rustfs` | Only `rustfs` |
| `version` | string | `latest` | `latest` or an exact release such as `1.0.5`; the same on every member |
| `runtime` | string | `native` | `native` or `docker` (Linux only) |
| `topology` | string | `standalone` | `standalone` (1 member) or `distributed` (2–32 Linux members, the same number of drives each, 4–64 drives in total) |
| `members` | array | `[]` | Required unless the shorthand below is used. `[{server_id, data_paths, endpoint_host?}]`. `data_paths`: 1–16 absolute directories per node (Windows: 1). `endpoint_host`: host the control plane and the other members reach the node on; default the address the node's agent connects from, or its hostname for a node that never connected |
| `server_id`, `data_path`, `endpoint_host` | string | none | Standalone shorthand for `members: [{server_id, data_paths: [data_path], endpoint_host}]`, used when `members` is empty |
| `require_mount` | bool | `false` | Refuse install and start unless every data path is a mount point |
| `s3_port` | number | `9000` | 1024 or higher; the same on every member, which also replicate over it |
| `console_port` | number | none | RustFS console, different from `s3_port`; omitted means the console is off |
| `environment_id` | string | none | The environment the store belongs to. Must be in your workspace (`404 environment not found`) and needs `deploy` on it. Omitted means a shared store |

One `storage.action` install job starts per member, in parallel. Response:
`202 {store, job, jobs}`, where `job` is the first job and `jobs` all of them.
Errors: `400` validation (topology, drive counts, Windows in a distributed
store), `403 step_up_required`, `403` without `deploy` on `environment_id`,
`404 environment not found`, `409` name, port or data path already taken on the
node.

**Connect body** (`POST /object-stores/connect`):

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `name` | string | required | Display name |
| `provider` | string | `s3` | `aws`, `s3`, `rustfs`, `minio`, `ceph`, `r2`, `wasabi`, `b2`, `other`. The API also accepts `garage` as a label for an existing Garage service; the console does not list it, so choose **S3-compatible** there |
| `endpoint` | string | required | `http(s)://host[:port]`, no path |
| `region` | string | `us-east-1` | Signing region |
| `path_style` | bool | `false` | Path-style bucket addressing |
| `access_key` | string | required | Stored in the vault as the item username |
| `secret_key` | string | required | Sealed in the vault; never returned |
| `bucket` | string | none | For bucket-scoped keys without `ListBuckets`: access is checked on this bucket |
| `environment_id` | string | none | The environment the store belongs to. Must be in your workspace (`404 environment not found`) and needs `deploy` on it. Omitted means a shared store |

Errors: `400` validation, `403 step_up_required`, `403` without `deploy` on
`environment_id`, `404 environment not found`, `502 store_unreachable`
(transport) or `502 store_error` (the S3 service refused).

**Adopt body** (`POST /object-stores/adopt`):

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `name` | string | required | Display name, 1–80 characters |
| `server_id` | string | required | Node with the App Store `rustfs` installation |
| `endpoint_host` | string | the address the node's agent connects from | Host the control plane reaches the node on. Falls back to the hostname for a node that never connected |
| `environment_id` | string | none | The environment the store belongs to. Must be in your workspace (`404 environment not found`) and needs `deploy` on it. Omitted means a shared store |

The store gets slug `rustfs`, runtime `docker`, topology `standalone`, the
installation's release, and its `port` and `console_port` parameters (default
`9000` and `9001`) as `s3_port` and `console_port`. Its data path is
`/var/lib/forge/pkgs/rustfs`. Status is `adopting` until the `storage.action`
job finishes. On success the job seals the container's key in the vault item
`credential_id` and deletes the App Store installation record. The key is not
in the job output. An adopted store refuses `install` (repair); `uninstall`
removes the container `forge-rustfs` and `/etc/forge/pkgs/rustfs.env` and
keeps the data.

Errors: `400` name or endpoint host, `403 step_up_required`, `403` without
`deploy` on `environment_id`, `404` server not found, `environment not found` or
`no App Store RustFS installation on this node`, `409` already adopted,
or another store on the node uses the slug `rustfs` or the S3 port.

**Owner body** (`PUT /object-stores/{id}/owner`):

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `environment_id` | string \| null | `null` | The environment the store is to belong to. `null` clears the owner and makes the store shared |
| `force` | bool | `false` | Assign the store although buckets of it are attached to services of other environments |

Assigning needs `storage.manage` (MFA) on the store and `deploy` on the
target environment. Clearing needs `storage.manage` only. The response is
`200` with the store object. The audit event `storage.owner.set` carries
`store_id`, `name`, `from` and `to` (environment ids or `null`), `forced`
(bool) and `affected`.

The roles of the owning environment reach every bucket of the store. Forge
therefore refuses to assign a store while buckets of it are attached to
services of environments other than the new owner
(`409 attached_elsewhere`; the detail names up to ten of them). With
`"force": true` the store is assigned, and `affected` in the audit event
lists each such attachment as
`{service_id, service, environment_id, bucket, access_key}`. Clearing
the owner never needs `force`.

A store has one owner. Changing or clearing it moves no data and leaves
existing access keys and attached buckets as they are.

```sh
curl -sS -b cookies.txt -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -X PUT https://<forge-host>/api/v1/object-stores/<store-id>/owner \
  -d '{"environment_id": "<environment-id>"}'
```

| Status | When |
| --- | --- |
| `200` | The owner is set or cleared; returns the store object |
| `403` | `storage.manage` missing on the store, or `deploy` missing on the target environment |
| `403 step_up_required` | The session is not MFA-verified |
| `404` | Store not found, or `environment not found`: the environment does not exist in your workspace |
| `409 attached_elsewhere` | Buckets of the store are attached to services of other environments and `force` is not set |

**Key body** (`POST /object-stores/{id}/keys`):

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `name` | string | required | What the key is for, 1–32 of letters, digits, `-`, `_`, `.` |
| `bucket` | string | required | Existing bucket the key is limited to |
| `access` | string | `readwrite` | `read` or `readwrite` |

The key is a RustFS service account of the root user with an inline policy:
`s3:ListBucket`, `s3:GetBucketLocation`, `s3:ListBucketMultipartUploads` on
`arn:aws:s3:::<bucket>`, and `s3:GetObject` on `arn:aws:s3:::<bucket>/*`; for
`readwrite` also `s3:PutObject`, `s3:DeleteObject`, `s3:AbortMultipartUpload`
and `s3:ListMultipartUploadParts`. The vault item is named
`<store name> / <bucket> (<name>)`: username = access key, URL = store
endpoint, linked to the member nodes, tags `object-storage`,
`objstore:<store-id>`, `bucket:<bucket>`, `access:<access>`. Reveal the secret
with `POST /vault/items/{id}/reveal`.

**Key object** (`GET /object-stores/{id}/keys`):

| Field | Type | Notes |
| --- | --- | --- |
| `access_key` | string | |
| `name`, `description` | string \| null | As the store reports them |
| `status` | string \| null | Account status as the store reports it |
| `expiration` | string \| null | As the store reports it |
| `managed_by_forge` | bool | `true` when a vault item for this key exists (Forge issued it) |
| `vault_item_id` | string \| null | Vault item with the secret; `null` for keys Forge did not issue |
| `bucket`, `access` | string \| null | From the vault item; `null` for keys Forge did not issue |

Key errors:

| Status | When |
| --- | --- |
| `400` | Invalid key name, bucket name, `access`, or access key in the path |
| `403 step_up_required` | `storage.manage` without an MFA-verified session |
| `404` | Store not found; the bucket does not exist (create); `no Forge-issued key with that access key` (rotate) |
| `409` | Connected store or a provider other than RustFS (`access keys are issued for managed RustFS stores`); revoking the root key |
| `502 rotation_failed` | The new key did not work on the bucket. Forge deleted it; the old key and the vault item are unchanged |
| `502 store_error`, `502 store_unreachable` | The store refused the admin call or cannot be reached. On rotate after the vault update, the new key is in the vault and the old key still exists; revoke it |

**Store object:**

| Field | Type | Notes |
| --- | --- | --- |
| `id`, `name`, `slug` | string | |
| `limited` | bool | `false` for this object; `true` for the limited store object below |
| `environment_id` | string \| null | The environment that owns the store; `null` for a shared store |
| `provider` | string | |
| `mode` | string | `managed` or `external` |
| `region`, `path_style` | string, bool | |
| `version`, `deployed_version` | string | Requested and installed release |
| `runtime` | string | `native`, `docker` or `external` |
| `topology` | string | `standalone`, `distributed` or `external` |
| `s3_port`, `console_port` | number, number \| null | |
| `endpoint` | string | S3 endpoint of the first member, or the external endpoint. Distributed stores fail over to the first reachable member |
| `credential_id` | string | Vault item |
| `status` | string | `installing`, `adopting`, `running`, `stopped`, `failed`, `error`, `removing`, `starting`, `stopping`, `restarting`, `checking`, `missing`, `unknown`, `degraded` (some members of a distributed store run), `connected` |
| `last_error`, `last_job_id` | string \| null | |
| `created_at`, `updated_at` | number | Epoch ms |
| `members` | array | `[{server_id, hostname, data_paths, endpoint_host, require_mount, status, capacity_bytes, free_bytes, dedicated, observed_at}]`; empty for external stores |

**Limited store object.** Returned by `GET /object-stores` and
`GET /object-stores/{id}` when your access comes only from the environment
that owns the store, or only from a live bucket grant, not from the store's
node or workspace. It has no other fields:

| Field | Type | Notes |
| --- | --- | --- |
| `id`, `name`, `slug` | string | |
| `limited` | bool | Always `true` |
| `by_grant` | bool | `true` when you reach the store through bucket grants only; `false` when through the owning environment |
| `environment_id` | string \| null | The environment that owns the store. Always `null` when `by_grant` is `true` |
| `provider`, `mode`, `topology`, `status` | string | As in the store object |
| `endpoint` | string | S3 endpoint. An empty string when `by_grant` is `true` |
| `members` | array | `[{hostname, status}]`. Empty when `by_grant` is `true` |

The role checks come first: a caller who holds a role and a grant gets the
view of the role.

### Bucket grants

A bucket grant lets one account work with the objects of one bucket through
Forge, without a role on the store, its node, the workspace or its
environment. It issues no S3 key. See
[Bucket grants](/docs/concepts/object-storage/#bucket-grants) and
[Grant a person access to a bucket](/docs/guides/grant-bucket-access/).

Creating, revoking and listing a store's grants need `storage.manage` on the
store (roles **Operator**, **Admin**, **Platform admin**) and an MFA-verified
session. A grant never opens these three endpoints.

| Method | Path | Authorization and behaviour |
| --- | --- | --- |
| GET | `/object-stores/{id}/grants?all=&limit=` | `storage.manage` (MFA) — the grants on the store, newest first → `{grants: [Grant]}`. `all` (bool, default `false`): include expired and revoked grants; without it only live grants. `limit` (integer, default `200`, clamped to 1–500): how many grants to return |
| POST | `/object-stores/{id}/grants` | `storage.manage` (MFA) — body below → `201 Grant`. The grant counts from the holder's next request. Audited as `storage.grant.create` with `grant_id`, `store_id`, `name` (the store's), `user_id`, `user`, `bucket`, `access`, `expires_at`, `never_expires` and `note`. A request that is allowed but fails (unknown bucket or account, a grant that exists, an invalid expiry, an unreachable store) is audited with the outcome `failure` and `store_id`, `name`, `user` and `bucket` as given (at most 128 characters each), `access` and `error` (the error code and status, such as `not_found (404)`) |
| DELETE | `/object-stores/{id}/grants/{grant_id}` | `storage.manage` (MFA) → `204`. The holder's next request on the bucket is refused. Revoking a revoked grant answers `204` and keeps the first time. `404` for a grant of another store. Audited as `storage.grant.revoke` with `grant_id`, `store_id`, `name`, `user_id`, `user`, `bucket`, `access` and `reason` (`revoked`; `bucket deleted` or `bucket recreated` when Forge revoked it with a bucket change) |
| GET | `/object-stores/grants/mine` | Signed in — the caller's own live grants, on stores that still exist → `{grants: [{id, store_id, store, bucket, access, expires_at, status}]}`: the fields of the same name of the grant object, without the note, the issuer and the dates. Empty for a disabled account and for an AI connector (MCP), for which grants do not count |

**Grant body** (`POST /object-stores/{id}/grants`). Unknown fields are
rejected.

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `user` | string | required | The account: a user id, a username or an email address, tried in that order. Usernames and email addresses match without regard to case. `user_id` is accepted as another name for this field. A service account can be named |
| `bucket` | string | required | A bucket of this store. It must exist; the name is matched exactly |
| `access` | string | `read` | `read` (list and download objects) or `readwrite` (also upload and delete objects) |
| `lifetime_minutes` | integer | none | Lifetime from now in minutes, `1` to `5270400` (10 years). Not together with `expires_at` |
| `expires_at` | integer | none | When the grant ends, in epoch milliseconds. Must be in the future. Not together with `lifetime_minutes` |
| `note` | string | `""` | Why the grant exists. Control characters are dropped and the text is cut to 200 characters |

Without `lifetime_minutes` and `expires_at` the grant never expires.

```sh
curl -sS -b cookies.txt -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -X POST https://<forge-host>/api/v1/object-stores/<store-id>/grants \
  -d '{"user": "<user>", "bucket": "<bucket>", "access": "read", "lifetime_minutes": 480}'
```

**Grant object:**

| Field | Type | Notes |
| --- | --- | --- |
| `id` | string | Grant id |
| `store_id`, `store` | string | The store's id and name |
| `user_id`, `user` | string | The holder's id and sign-in name: the username, else the email address, else the id |
| `bucket` | string | |
| `access` | string | `read` or `readwrite` |
| `created_by` | string | Principal that issued the grant, `user:<id>` |
| `created_by_name` | string \| null | That account's sign-in name; `null` when the account is not known |
| `created_at` | number | Epoch ms |
| `expires_at` | number \| null | Epoch ms; `null` when the grant never expires |
| `never_expires` | bool | |
| `revoked_at` | number \| null | Epoch ms; `null` unless revoked |
| `note` | string | |
| `status` | string | `active`, `expired` or `revoked`. A revoked grant stays `revoked` after its expiry |

```json
{
  "id": "<grant-id>",
  "store_id": "<store-id>",
  "store": "media",
  "user_id": "<user-id>",
  "user": "ada",
  "bucket": "uploads",
  "access": "read",
  "created_by": "user:<admin-user-id>",
  "created_by_name": "root",
  "created_at": 1794590000000,
  "expires_at": 1794618800000,
  "never_expires": false,
  "revoked_at": null,
  "note": "",
  "status": "active"
}
```

**Rules:**

- A grant is live while it is not revoked, not expired by the server's clock
  and its holder's account is not disabled. It is read when a request
  starts, so revoking and expiry take effect with the next request; a
  download that is already running finishes.
- An account holds at most one live grant per store and bucket. To change
  the access or the expiry, revoke the grant and create a new one.
- Grants count for the account's sessions, its API keys and the tokens of a
  service account that holds the grant. They do not count for AI connectors
  (MCP), and not for a connector's request that a person approved: that
  request runs with the person's roles only.
- Using a grant needs no MFA-verified session.
- Deleting the store (an uninstall that removes it, or `disconnect`) deletes
  its grants. `DELETE …/buckets/{bucket}` revokes the live grants on that
  bucket. `POST …/buckets`, and an attach that creates the bucket, revoke
  any grant still live for the new bucket's name. A bucket deleted and
  created again outside Forge keeps a grant that is still live.
- A grant holder gets `502 store_unreachable` with
  `the store can't be reached` for every failure of the store, `404` with
  `no such bucket or object` and `400` with
  `the bucket name or the key is not valid`. The store's address and its
  own error text are not shown.

Grant errors:

| Status | When |
| --- | --- |
| `400` | Both `lifetime_minutes` and `expires_at` are set; the body is not valid JSON |
| `403` | `storage.manage` missing on the store |
| `403 step_up_required` | The session is not MFA-verified |
| `404` | Store not found; `no bucket … in this store`; `unknown or disabled user` (one answer for an account that does not exist and one that is disabled); `grant not found` (also for a grant of another store) |
| `409` | The account already holds a live grant for this bucket (the detail names it) |
| `422` | `expires_at` is not in the future; `lifetime_minutes` is outside 1 minute to 10 years; `user` is empty; a required field is missing, a field is unknown or has the wrong type, or `access` is not `read` or `readwrite` |
| `502 store_error`, `502 store_unreachable` | The store refused the bucket listing that checks `bucket`, or cannot be reached. Nothing is saved |

## Files

Browse, view, edit and transfer files on a node. All paths are
slash-separated, start with `/` and are relative to the root; `.` and `..`
are rejected. On Windows the node root's first component is the drive, as in
`/C:/Users`. `root` defaults to `node`. See
[File explorer](/docs/concepts/file-explorer/) for the checks behind every call.

Permissions: reads need `file.read`. Creating, saving and uploading need
`file.read` and `file.write`. Deleting and purging need `file.read` and
`file.delete`. Moves and renames need all three. Restoring from the trash
needs `file.write`. The `node` root also needs `file.node`, and approving
roots needs `file.admin`. `file.write`, `file.delete` and `file.admin` need
an MFA session (`403 step_up_required` otherwise). Read-only roots refuse
every change.

### Roots

| Method | Path | Description |
| --- | --- | --- |
| GET | `/servers/{id}/files/roots` | `{server_id, os, mfa, roots: [{id, kind, label, base, read_only, permissions: {read, write, delete, admin}, container?, service_id?}]}`. Only the roots the caller may open |
| POST | `/servers/{id}/files/roots` | `{kind: "service" \| "container", label, path, container?, read_only = true}` → `201` with the root. `container` is the full 64-character container id. The directory must exist and must not be a protected path. `409` if the node already has a root of that kind on that folder (an app folder approved as a plain directory counts as the app's root); the message names the existing root. Exact duplicates stored by older versions are listed once (the oldest) and stay usable by id until removed |
| DELETE | `/servers/{id}/files/roots/{root}` | Removes an approved root (`204`). Files are not touched |

App roots (Linux nodes): a `service` root whose `path` is exactly a
native app's folder, `/srv/forge/apps/<environment>/<app>`, is stored as an
[app root](/docs/concepts/file-explorer/#app-roots). Every call on it runs on the
node as the app's account. It keeps `kind: "service"` and `base` is the
app's folder; `file.root.create` records its `spec` as
`{"kind":"app","environment":"<environment>","app":"<app>"}`. Roots approved
earlier on an app's folder are treated the same way when used.

| Request | Response |
| --- | --- |
| `path` is `/srv`, `/srv/forge`, `/srv/forge/apps` or `/srv/forge/apps/<environment>` | `400 bad_request`: `` `<path>` holds native apps' folders; approve one app's folder (/srv/forge/apps/<environment>/<app>) instead `` |
| `path` is inside an app's folder, such as `/srv/forge/apps/<environment>/<app>/public` | `400 bad_request`: `` `<path>` is inside a native app's folder; approve the app's folder itself (/srv/forge/apps/<environment>/<app>) `` |
| The app is not deployed on the node | `422 file_error`: `<environment>/<app> has no account on this node yet; deploy the app first` |
| Any call on an earlier root inside or around app folders | `400 bad_request` with the messages above |

On an app root, `chown` is refused (`422 file_error`), `chmod` removes the
setuid and setgid bits, the `.forge` folder and the app's folder itself
cannot be changed, and links are never followed. File audit events on an
app root carry two more detail fields:

| Field | Type | Description |
| --- | --- | --- |
| `app` | string | `<environment>/<app>`, from the control plane |
| `as_reported` | string | The account the node says it ran the operation as, such as `<environment>-<app>`. Present on successful calls; at most 64 characters |

### Reading

| Method | Path | Description |
| --- | --- | --- |
| GET | `/servers/{id}/files/list?root=&path=&offset=&limit=&hidden=` | One page of a directory, sorted by name. `limit` 1–1000 (default 500). Returns `{path, offset, total, truncated, skipped_names, entries}` |
| GET | `/servers/{id}/files/stat?root=&path=` | `{path, entry, version}`; links are reported, not followed |
| GET | `/servers/{id}/files/read?root=&path=&offset=&length=&version=` | Up to `length` bytes (1–2097152, default 2097152) from `offset`; the server reads the node in 256 KiB pieces in one round trip, all pinned to the version of the first: `{path, offset, length, size, eof, version, modified_ms, content_b64}`. `409` if `version` no longer matches |
| GET | `/servers/{id}/files/hash?root=&path=` | `{sha256, size, version}` |
| GET | `/servers/{id}/files/download?root=&path=` | The file as an `application/octet-stream` attachment with `Content-Security-Policy: sandbox`, up to 512 MiB. The transfer fails if the file changes during it |

Each entry is `{name, kind, size, modified_ms, mode?, owner?, group?,
link_target?, mount, protected, readonly, hidden}`. `kind` is `file`, `dir`,
`symlink` or `other`. `mode`, `owner` and `group` are Linux only.

### Changing

| Method | Path | Body | Description |
| --- | --- | --- | --- |
| POST | `/servers/{id}/files/mkdir` | `{root?, path, parents = false}` | Creates a directory. Without `parents` the parent must exist, and the error names the first missing folder in full (`parent folder /srv/forge does not exist`). With `parents: true`, every missing folder on the way is created and an existing folder is not an error. Returns `{path, created}` (folders created) |
| POST | `/servers/{id}/files/save` | `{root?, path, content_b64, version?}` | Saves up to 2 MiB atomically. Without `version` the file must not exist; with it, `409` if the file changed since. Returns `{path, size, version, replaced}` |
| POST | `/servers/{id}/files/move` | `{root?, from, to, overwrite = false}` | Renames or moves within the root. Across filesystems: copy, then remove (1 GiB limit) |
| POST | `/servers/{id}/files/copy` | `{root?, from, to, overwrite = false}` | Copies a file or directory tree within the root (10,000 entries, 1 GiB) |
| POST | `/servers/{id}/files/delete` | `{root?, path, permanent = false}` | Moves to the trash, or removes for good. Returns `{trash_id?}` |

### Uploads

| Method | Path | Body | Description |
| --- | --- | --- | --- |
| POST | `/servers/{id}/files/uploads` | `{root?, path, size, overwrite?}` | Starts an upload of up to 50 GiB. `overwrite` is `"never"` (default), `"always"` or `{version}`. With `"never"`, an existing `path` is refused with `409` before any byte is sent. Returns `201 {id, path, size, chunk_size, parallel, expires_at}`. `chunk_size` is 2 MiB; send chunks of that size. `parallel` (3) is how many chunks you may send at once: the server writes them to the node in offset order |
| PUT | `/servers/{id}/files/uploads/{upload}?offset=` | raw bytes | One chunk of at most `chunk_size` bytes at `offset`; the server writes it to the node in 256 KiB pieces in one round trip. A chunk that arrives before the ones before it waits for them (up to 2 minutes, then `409`); at most `parallel` chunks of an upload wait at once, and more are refused with `429`. Only the upload's creator may send chunks, within its size. Returns `{received}` |
| GET | `/servers/{id}/files/uploads/{upload}` | | `{id, path, size, received, expires_at}`: where to resume |
| POST | `/servers/{id}/files/uploads/{upload}/commit` | `{sha256}` | Verifies the size and SHA-256, then moves the file into place. A `409` (the target exists or changed) ends the upload and removes the partial file; after a checksum mismatch you may commit again with the right checksum |
| DELETE | `/servers/{id}/files/uploads/{upload}` | | Cancels the upload and removes the partial file (`204`) |

Only the user who started an upload can continue it. Permissions are
checked on every chunk and at commit. Unfinished uploads expire after
24 hours.

### Trash, revisions and search

| Method | Path | Description |
| --- | --- | --- |
| GET | `/servers/{id}/files/trash?root=` | `{entries: [{id, path, deleted_at, size, kind}]}` for this root |
| POST | `/servers/{id}/files/trash/restore` | `{root?, id, overwrite = false}`: puts the entry back where it was |
| POST | `/servers/{id}/files/trash/purge` | `{root?, id?}`: removes one entry, or all of this root's entries |
| GET | `/servers/{id}/files/revisions?root=&path=` | `{path, revisions: [{id, saved_at, size}]}`, newest first |
| GET | `/servers/{id}/files/revision?root=&path=&revision=&offset=&length=` | Content of a saved revision, like `read` |
| POST | `/servers/{id}/files/search` | `{root?, path?, spec}`; see below |

| `spec` field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | string | `""` | Glob on file names (`*.conf`) |
| `content` | string | `""` | Text to find, or a regular expression with `regex` |
| `regex` | bool | `false` | Treat `content` as a regular expression |
| `case_sensitive` | bool | `false` | |
| `whole_word` | bool | `false` | |
| `context` | int | `0` | Lines before and after each match, 0–5 |
| `include`, `exclude` | string[] | `[]` | Globs on the relative path, up to 32 each (`**` spans folders) |
| `max_depth` | int | `32` | At most 64 |
| `max_file_size` | int | `1048576` | Bytes; larger files are skipped. At most 8 MiB |
| `modified_since` | int | `0` | Epoch ms |
| `hidden` | bool | `false` | Include dotfiles and hidden files |
| `after` | string | | Resume after this relative path (`next` of a previous answer) |

Give `name`, `content` or both. The answer is `{path, status, next, files:
[{path, size, modified_ms, matches?: [{line, text, before?, after?}]}],
matches, scanned_files, scanned_bytes}`. `status` is `complete`,
`truncated` or `timeout`; `next` continues a search that stopped early. One
search per user per node runs at a time.

### Permissions, archives and bulk actions

| Method | Path | Body | Description |
| --- | --- | --- | --- |
| POST | `/servers/{id}/files/chmod` | `{root?, path, mode, recursive = false, dry_run = false}` | Sets permission bits (Linux node, directory and app roots; on app roots without setuid and setgid). `mode` is octal (`"0644"`). Needs `file.admin` (MFA). Returns `{changed, unchanged, skipped, sample: [{path, before, after}]}`; `dry_run` only counts |
| POST | `/servers/{id}/files/chown` | `{root?, path, owner?, group?, recursive = false, dry_run = false}` | Sets owner and/or group by name. Refused on app roots. Needs `file.admin` (MFA). Same answer as `chmod` |
| GET | `/servers/{id}/files/acl?root=&path=` | | `{path, acl, mode?}`: the Windows ACL as `icacls` prints it, or the POSIX mode on Linux. Read-only |
| POST | `/servers/{id}/files/compress` | `{root?, sources, dest, format?}` | Packs 1–500 files and folders into a new `.zip` or `.tar.gz` (`format` follows the name); never overwrites. Returns `{dest, size, entries, bytes, skipped}` |
| POST | `/servers/{id}/files/extract` | `{root?, archive, dest, replace = false}` | Unpacks `.zip`, `.tar`, `.tar.gz` or `.tgz` into a new folder `dest` (`409` if it exists). With `replace: true`, an existing folder `dest` is replaced: the archive is unpacked next to it, then swapped in (in one atomic step on Linux, so a site served from `dest` never misses its files), and the old content is deleted. `replace` also needs `file.delete`. Returns `{dest, entries, bytes, skipped, replaced}` |
| POST | `/servers/{id}/files/bulk` | `{root?, action, paths, to?, overwrite = false}` | `action` is `copy`, `move` (both need `to`, a folder), `trash` or `delete`, for 1–500 paths. Returns `{ok, failed, results: [{path, ok, error?}]}` |
| POST | `/servers/{id}/files/archive` | `{root?, paths, format = "zip"}` | Packs a selection into a temporary archive for download (read access only). Returns `{temp_id, ext, size, entries}` |
| GET | `/servers/{id}/files/archive/{temp_id}?root=` | | Streams the temporary archive as an attachment, then deletes it. Unclaimed archives are removed after an hour |

Extraction refuses archives with absolute paths, drive letters or `..` in an
entry name, more than 20,000 entries, more than 4 GiB of content, or content
more than 200 times the archive's size. Links, hard links and device files in
archives are skipped and listed in `skipped`. Files get mode `0644` and
folders `0755`; modes and owners stored in the archive are ignored.

### Temporary SFTP access

| Method | Path | Description |
| --- | --- | --- |
| GET | `/servers/{id}/files/sftp` | `{enabled, protocol, host, port, fingerprint, max_hours, allow_never_expiry, can_never_expire}`; only `{enabled: false}` when the gateway is off. `can_never_expire` is `true` when `allow_never_expiry` is on and you hold `file.admin` on the node |
| GET | `/files/grants?server=&status=&q=` | `{grants: [...]}` across the fleet: on each node where you hold `file.read`, every grant with `file.admin`, else your own; other nodes are left out without a denial. Each grant also has `server_name`. `status` is `active`, `expired` or `revoked` (`400` otherwise); `q` matches the name, id, creator or node name, ignoring case |
| GET | `/servers/{id}/files/grants` | `{grants: [...]}`: every grant with `file.admin`, else your own. Tokens and their hashes are never returned |
| GET | `/servers/{id}/files/grants/{grant}` | One grant, visible as in the list (`404` otherwise) |
| POST | `/servers/{id}/files/grants` | Issues access (MFA). Returns `201 {grant, connection: {protocol, host, port, username, password, expires_at, fingerprint}}`. The password is in this answer only |
| PATCH | `/servers/{id}/files/grants/{grant}` | Changes a grant; the password stays (MFA; its issuer or `file.admin`, plus every right the result carries). Body: any [change field](#grant-changes). Returns the grant. Audited as `file.grant.update` with `{changes: {field: {before, after}}, sessions_closed}`. `409` for a revoked grant |
| POST | `/servers/{id}/files/grants/{grant}/reset-password` | New password (MFA; its issuer or `file.admin`). Returns `{grant, connection}` with the new password, in this answer only. Open sessions close. Audited as `file.grant.reset_password` |
| POST | `/servers/{id}/files/grants/{grant}/revoke` | Revokes (its issuer, or `file.admin`); open sessions close at once |

| Request field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | string | | What the access is for, 1–120 characters |
| `roots` | string[] | | Root ids (`node` or approved roots), 1–8 |
| `write` | bool | `false` | Uploads, folders, renames (`file.write`) |
| `delete` | bool | `false` | Deletes and replacing files (`file.delete`) |
| `expires_in_minutes` | int | | 1 up to `max_hours` × 60. Ignored with `never_expires` |
| `never_expires` | bool | `false` | No expiry. `403` unless `[sftp] allow_never_expiry` is on and you hold `file.admin`; the refusal is audited |
| `allowed_ips` | string[] | `[]` | Client IPs or CIDR ranges, up to 32; empty allows any |
| `max_sessions` | int | `1` | Sessions at once, 1–10 |
| `bandwidth_kbps` | int | | KiB/s per session |
| `transfer_limit` | int | | Total bytes the access may transfer |

A grant is `{id, workspace_id, server_id, name, roots, write, delete,
created_by, created_at, expires_at, revoked_at, allowed_ips, max_sessions,
bandwidth_kbps, transfer_limit, bytes_transferred, last_used_at, active,
status, never_expires}`. `status` is `active`, `expired` or `revoked`. A
grant that never expires has `never_expires: true` and `expires_at`
`253402300799999` (the end of year 9999). Returns `409` with
`sftp_disabled` when the gateway is off.

`owner` is the principal whose current rights the grant acts with. It starts
as `created_by`. A `PATCH` that adds a root, `write` or `delete` makes the
caller the owner (audited as `changes.owner` with `before` and `after`, and
open sessions close); other changes keep it. Every SFTP operation needs the
owner's current `file.read`, plus `file.write` for uploads and new folders
and `file.delete` for deletes and replacing files, on that node; otherwise
it is refused. The owner can manage the grant like its creator.

#### Grant changes

`PATCH /servers/{id}/files/grants/{grant}` takes any of these fields.
Absent fields stay as they are; unknown fields are refused (`400`).

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | string | 1–120 characters |
| `roots` | string[] | Replaces the roots, 1–8, each one you can open now |
| `write` | bool | Uploads, folders, renames (`file.write`) |
| `delete` | bool | Deletes and replacing files (`file.delete`) |
| `expires_in_minutes` | int | New lifetime counted from now, 1 up to `max_hours` × 60 |
| `never_expires` | bool | `true`: no expiry, same rules as on create. `false` on a never-expiring grant needs `expires_in_minutes` |
| `allowed_ips` | string[] | Replaces the allowed client addresses; `[]` allows any |
| `max_sessions` | int | 1–10 |
| `bandwidth_kbps` | int or `null` | KiB/s per session; `null` removes the cap |
| `transfer_limit` | int or `null` | Total bytes; `null` removes the allowance |

When a change takes anything away (a root, `write`, `delete`, an address,
sessions, bandwidth, transfer allowance or lifetime), the gateway closes the
grant's open sessions at once. Other changes apply to new sessions.

### Errors

| Status | Meaning |
| --- | --- |
| `400` | Invalid path, root, limit, length, pattern or upload chunk |
| `403` | Not allowed, read-only root, protected path, or the node refused access. `step_up_required` when MFA is missing |
| `404` | Unknown root, path, upload or trash entry |
| `409` | The file changed (`version`), the destination exists, or another change to the path is in progress |
| `413` | Too large (download, save, upload, copy) or the disk is full |
| `422` | A link, a directory where a file was expected, a name Windows rejects, or a failed checksum |
| `429` | A search of yours is already running on this node, or the node is busy with two searches |
| `501` | The node's agent does not allow the `files` command |
| `504` | The node did not answer (offline, or an agent too old for the file explorer) |

## Containers

| Method | Path | Permission |
| --- | --- | --- |
| POST | `/servers/{id}/containers` | `{op, id?, image?, tail?}` — reads (`info`, `list`, `images`, `inspect`, `logs`, `stats`) need `server.manage`; `start`, `stop`, `restart`, `remove`, `pull` need `container.manage`. Returns `{job_id, status, result?, error?}` |
| POST | `/servers/{id}/containers` with `{op: "inspect", id, reveal_env: true}` | `container.inspect_env` (MFA) — `inspect` normally returns `env_keys` only. With `reveal_env` the result also carries `env: [{key, value}]`. The values are not stored with the job. Audited as `container.env.reveal` with the node and container id, never the values. `409` with code `agent_update_required` when the node's agent predates this. Denials and MFA step-up come back as `403` before any job runs |
| GET (WebSocket) | `/servers/{id}/containers/{container_id}/shell?cols=&rows=&login=` | `container.exec` (no MFA) — an interactive shell inside a running container, same frames as [`/servers/{id}/terminal`](#terminal-browser-ssh). **Without `login`** (the default) it runs through the node's agent and root helper: no terminal account is needed; the shell runs as the image's default user with `bash`, or `sh` when the image has no bash. `ready` carries `session`, `container` and `path: "agent"`; `closed` adds `exit_code` (number, or `null` when unknown); a node-side failure (for example `no container `<id>` on this node`, or Docker refusing a stopped container) arrives as `error`. `409` when the node runs Windows (`container shell is not supported on Windows nodes`), is offline, has not reported its facts yet, runs an agent without container shells (code `agent_update_required`), or when you (8) or the node (8) are at the limit; an agent that does not answer within 15 s gives `error` `the agent did not answer: update the agent …`. **With `login=<account>`** it takes the SSH path instead: Forge logs in as that account and runs `sudo -n docker exec -it <id>`; without passwordless `sudo` the shell prints the reason and exits; `ready` carries `login`, `host` and `session`. `container_id` is a Docker id or name (`[A-Za-z0-9][A-Za-z0-9_.-]{0,127}`, `400` otherwise). Sessions appear in `/terminal-sessions` with kind `container`. Audited as `container.exec` and `container.exec_close` |
| GET | `/servers/{id}/containers/{container_id}/shell/check?cols=&rows=&login=` | Same checks as opening the shell, as JSON: `200 {ok, path: "agent"}` without `login`, `200 {ok, path: "ssh", host, port, login}` with it; or `400` (invalid id), `403` (denied), `409` (the agent-path reasons above, or SSH endpoint conflicts) |

## Docker Engine API

A node's full [Docker Engine API](/docs/concepts/docker-engine-api/) over mutual
TLS. The control plane dials dockerd on the node; callers only call these
paths. Paths are under `/api/v1`.

| Method | Path | Permission | Description |
| --- | --- | --- | --- |
| GET | `/servers/{id}/docker/endpoint` | `server.manage` | The endpoint object (below). A node without the endpoint returns `state: "disabled"` and `null` fields |
| POST | `/servers/{id}/docker/endpoint` | `server.command` (MFA) | Enable. Body `{host?, port?, listen?, acknowledge_restart = false}` (unknown fields refused) → `202 {job_id}`, a `docker.endpoint.enable` job (steps `prepare`, `sign`, `apply`, `verify`). `host`: IP address or DNS name the control plane dials; default the address the node's agent connects from. `port`: default `2376`. `listen`: IP address dockerd binds; default `host` when it is one of the node's addresses. `acknowledge_restart`: accept that running containers stop while dockerd restarts (`live-restore` off); without it the job fails at `prepare`. `400` for an invalid `host`, a `port` below `1024`, or no known address and no `host`; `404` no such node. Audited as `docker.endpoint.enable` with the request |
| DELETE | `/servers/{id}/docker/endpoint?force=false` | `server.command` (MFA) | Disable → `202 {job_id}`, a `docker.endpoint.disable` job (steps `remove`, `forget`); the node's Docker CA is deleted. `force=true` skips the node (when it is gone). `409` when the endpoint is not enabled; `404` no such node. Audited as `docker.endpoint.disable` with `force` |
| POST | `/servers/{id}/docker/endpoint/check` | `server.manage` | Calls `/version` on dockerd now and records the versions or `last_error` → the endpoint object. A `pending` or `failed` endpoint becomes `enabled` when dockerd answers. `409` when there is no endpoint |
| GET | `/docker/catalog` | `view` on the platform | `[{method, path, class, action, transfer, op}]`: every forwarded call; see the [catalog](/docs/reference/docker-api-catalog/). `class` is `read`, `manage`, `exec` or `admin`; `action` is the Forge action before body checks; `transfer` is `inspect`, `stream` or `hijack` |
| GET, HEAD, POST, PUT, DELETE | `/servers/{id}/docker/engine/{path}` | The class's action on the node: `read` `server.manage`, `manage` `container.manage`, `exec` `container.exec`, `admin` `server.command` (MFA) | Forwards one Engine API call, as the Engine API reference describes it, to the node's dockerd and returns dockerd's status, headers (allow-listed) and body. `{path}` may start with a version (`v1.47/…`); the query string is passed on. Details below |

**Endpoint object**: `{state, host, port, engine_version, api_version,
ca_fingerprint, daemon_cert_not_after, last_error, updated_at}`. `state` is
`disabled`, `pending`, `enabled` or `failed`; `ca_fingerprint` is the
SHA-256 of the node's Docker CA certificate (hex); `daemon_cert_not_after`
and `updated_at` are epoch ms.

**Engine calls.** Before anything reaches the node:

1. The method and path must match the [catalog](/docs/reference/docker-api-catalog/).
   Otherwise `404` `docker_unknown_endpoint`. Dot or empty segments, encoded
   separators (`%2f`, `%2e`, `%5c`, `%00`), control characters, spaces, and
   paths over 1,024 characters answer `400`.
2. A query string over 8 KiB answers `400`.
3. For `inspect` entries the JSON body is read, at most 1 MiB (`413`
   `body_too_large` above that; `400` when it is not JSON). Host-access
   settings raise the call to `admin` (see
   [Host access](/docs/reference/docker-api-catalog/#host-access)).
4. The caller is authorized for the class: `403` `forbidden`, or `403`
   `step_up_required` for `admin` without a verified second factor. Denials
   are audited with `op`, `method`, `path` and `host_access`.
5. The endpoint must be `enabled`: otherwise `409`
   `docker_endpoint_unavailable` with the state and last error.

| Status | Code | When |
| --- | --- | --- |
| dockerd's | | The call was forwarded; dockerd's own status and body |
| `101` | | An upgraded stream (`hijack` entries) |
| `400` | `bad_request` | Bad path, query over 8 KiB, body not JSON, a `hijack` entry without `Upgrade`, or a connection that cannot be upgraded (HTTP/2) |
| `403` | `forbidden`, `step_up_required` | The class's action is denied, or needs MFA |
| `404` | `docker_unknown_endpoint`, `not_found` | Not in the catalog; no such node |
| `409` | `docker_endpoint_unavailable` | The endpoint is not enabled, `pending` or `failed` |
| `413` | `body_too_large` | An inspected body over 1 MiB |
| `502` | `docker_unreachable`, `docker_bad_answer` | dockerd could not be reached, or a redacted inspect answer was not JSON |

**Bodies.** `stream` request bodies pass through up to 8 GiB. Responses
stream (logs, `/events`, pulls, archives). For `hijack` entries (attach,
exec start, `/session`, `/grpc`) send `Connection: Upgrade` and
`Upgrade: tcp` (or a WebSocket handshake for `/containers/{id}/attach/ws`)
over HTTP/1.1; the server answers `101` and relays raw bytes both ways.

**Headers.** Request headers passed to dockerd: `content-type`,
`x-registry-auth`, `x-registry-config`, `accept`, `sec-websocket-key`,
`sec-websocket-version`, `sec-websocket-protocol`,
`sec-websocket-extensions`. Response headers passed back: `content-type`,
`api-version`, `docker-experimental`, `ostype`, `server`,
`docker-content-digest`, `x-docker-container-path-stat`,
`sec-websocket-accept`, `sec-websocket-protocol`,
`sec-websocket-extensions` (plus `connection` and `upgrade` on `101`). All
others are dropped.

**Redaction.** A successful `GET /containers/{id}/json` for a caller
without `container.inspect_env` returns every `Config.Env` entry as
`KEY=<redacted>` and adds `x-forge-redacted: Config.Env`.

**Audit.** Calls outside the `read` class are audited as `docker.api` on the
node with `{op, method, path, query (keys only), status, class,
host_access}`; `status` is `0` when dockerd could not be reached. Bodies
are never recorded.

## Compose stacks

Stacks are compose files in an environment, deployed per node (see
[Compose stacks](/docs/concepts/compose-stacks/)). Changes need `stack.manage` on
the environment; deploying a stack with host-access settings also needs
`stack.privileged` (administrators, MFA). `.env` values are sealed and never
returned; responses carry `env_keys` only.

| Method | Path | Permission | Description |
| --- | --- | --- | --- |
| GET | `/stacks?environment_id=&server_id=` | `view` (filtered) | `[{stack, nodes}]`. `server_id` keeps stacks with a part on, or created from, that node |
| POST | `/stacks/preview` | `stack.manage` on the environment and `server.manage` on the node | `{environment_id, server_id, name?, content, env?, dir?}` → the preview (below), plus `can_privileged`. The node must be in the environment's organization. Nothing is saved |
| POST | `/stacks` | `stack.manage` + `server.manage` on `server_id` | `{environment_id, server_id, name, content, env?, dir?, note?}` → `201 {stack, preview}`. `400` when a setting is never allowed; privileged stacks can be saved |
| POST | `/stacks/containers` | as `POST /stacks` | **Create container**: `{environment_id, server_id, dir?, start = true, container}` → `201 {stack, preview, jobs}` |
| GET | `/stacks/{id}` | `view` | `{stack, content, dir, nodes, preview}`; `preview` is `{error}` when the stack no longer plans (a node is gone) |
| PUT | `/stacks/{id}` | `stack.manage` | `{content?, env?, dir?, note?, version?}`. A changed `content` adds a revision; omitted `env` keeps the stored variables; `dir` only while not deployed. `version` is the stack's `updated_at` when you loaded it: if someone saved the stack since, the answer is `409` `stale_version` and nothing changes. Concurrent saves of the same version: one wins, the others get `409`. A deploy does not change `updated_at` |
| DELETE | `/stacks/{id}` | `stack.manage` | `204`; `409` while any node has a part |
| GET | `/stacks/{id}/revisions` | `view` | `[{stack_id, revision, sha256, note, author, created_at}]`, newest first |
| GET | `/stacks/{id}/revisions/{revision}` | `view` | One revision with `content` |
| POST | `/stacks/{id}/deploy` | `stack.manage` (+ `stack.privileged`) + `server.manage` on each node | `{revision?, services?, pull = false}` → `202 [Job]`: one `stack.action` job per node that runs a chosen service (with its `depends_on`); all nodes when `services` is empty. An older `revision` rolls back. Nodes that no longer have services are taken down |
| POST | `/stacks/{id}/services` | `stack.manage` | `{server_id, action, services?}`; `action` is `start`, `stop`, `restart` or `pull` → `202 Job` |
| POST | `/stacks/{id}/refresh` | `view` | Reads `docker compose ps` on each node → `202 [Job]` |
| GET | `/stacks/{id}/logs?server_id=&service=&tail=200` | `view` | `{job_id, logs}`, or `{job_id, status: "running"}` after 25 s. `tail` up to 10,000 |
| POST | `/stacks/{id}/down` | `stack.manage` | `{volumes = false, server_id?}` → `202 [Job]`. `volumes: true` also removes named volumes (data is lost) |
| POST | `/stacks/{id}/expose` | `stack.manage` + `workspace.manage` | `{service, port, domains, tls?, acme_email?, cert_id?, edges, cluster_id?}` → `201 {site}`: a site whose upstream is the service. Apply it from **Ingress** |

**Stack**: `{id, environment_id, name, kind: compose|container, default_server, dir, revision, deployed_revision, env_keys, created_by, created_at, updated_at}`.
**Node part**: `{stack_id, server_id, services, selected, revision, status, state: [{service, name, state, health, status, image, exit_code, ports}], last_job_id, last_error}`; `status` is `deploying`, `deployed`, `failed`, `stopping`, `starting`, `pulling` or `removing`.
**Preview**: `{services: [{name, image, command, ports: [{host_ip, published, target, protocol, public}], volumes, environment: [{key, value}], depends_on, healthcheck, restart, expose, nodes, policy: allowed|privileged|blocked, findings}], nodes: [{server_id, hostname, services, internal_ip, ports, findings}], findings, privileged, blocked}`. Values of secret-looking variables are masked; `${VAR}` references are shown as written. Every field comes from the file as written, and `.env` values are replaced by `${VAR}` in `findings`, `ports` and errors.
**Create container** `container`: `{name, image, command?, entrypoint?, ports: [{container, host?, protocol = tcp, public = false}], env: [{key, value}], volumes: [{source, target, read_only}], restart = unless-stopped, cpus?, memory_mb?, network?, healthcheck?: {test, interval = "30s"}, labels?, expose?: http|https}`. `network: host` or `none` becomes `network_mode`; `host` needs `stack.privileged` to start. Variables are stored in the sealed `.env` and referenced as `${KEY}`.

Services deployed from a stack are listed under [Services](#services)
with runtime `compose-service`; `PUT`, `DELETE`, placement, deploy and
actions on them return `409`.

## Terminal (browser SSH)

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/servers/{id}/ssh` | `view` — endpoint, pinned host key, CA public key |
| PUT | `/servers/{id}/ssh` | `server.manage` + `server.command` (MFA) — `{host, port, host_key_fingerprint}`; `400` when the key or `host:port` belongs to another node, the host is another node's agent address, or the node's agent reports other host keys |
| GET | `/ssh/ca` | signed in — the SSH user-CA public key |
| GET (WebSocket) | `/servers/{id}/terminal?login=&cols=&rows=` | `terminal.open` (MFA). Console (cookie): first frame `{"type":"auth","csrf":…}`. CLI (bearer token in the `Authorization` header, never the URL): no `auth` frame. Binary frames carry keystrokes/output; text frames `resize`, `ping` → `ready` (with `session`), `pong`, `error`, `closed` (with `reason`, `revoked` when ended through `/terminal-sessions`) |
| GET (WebSocket) | `/servers/{id}/exec?login=` | `terminal.open` (MFA). One command without a terminal. First frame (after `auth` for cookies) `{"type":"exec","argv":[…]}` (each argument quoted for the POSIX shell) or `{"type":"exec","command":"…"}` (a shell command line), at most 64 KiB. Output: binary frames whose first byte is `1` (stdout) or `2` (stderr). Input: binary frames are stdin, `{"type":"eof"}` ends it. Status: `{"type":"exit","code":n}` (128 + signal when killed), then `closed`. Audited as `terminal.exec` with the command (first 1,000 characters) and `terminal.exec_close` with the status |
| GET | `/servers/{id}/terminal/check?login=` | Same checks as opening a terminal, as JSON (`403 step_up_required`, `409` no SSH endpoint, …). The console and the CLI call it first because a failed upgrade hides the reason |
| GET | `/terminal-sessions` | signed in — your open terminal and exec sessions `{sessions: [{id, user, server_id, login, kind, client, started_at}]}`; `?all=true` lists everyone's and needs `user.manage` |
| POST | `/terminal-sessions/{id}/revoke` | the session's owner, or `user.manage` — ends it within a second (`closed`, reason `revoked`). Others get `404`. Audited as `terminal.revoke` |

A terminal or exec request that carries both a bearer token and a session
cookie is refused with `400`. Sessions live in the server process: a server
restart ends them, and the list shows sessions of this server only.

## Port tunnels

[Port tunnels](/docs/concepts/port-tunnels/) forward a local port to a service's
port on a node. Paths are under `/api/v1`. The WebSocket endpoints take the
bearer token in the `Authorization` header, never in the URL, and refuse a
foreign `Origin`. AI connector sessions are refused.

A tunnel's **target** is a service from the port catalogue, a node's SSH
port or, for administrators, a raw port:

| `target.kind` | `target.ref` | Allowed by |
| --- | --- | --- |
| `app` | The service's name, id or `<environment>/<name>` | `network.tunnel` (MFA) on the environment, or a grant |
| `stack` | `<stack>/<service>` (or the stack with `target.service`) | `network.tunnel` (MFA) on the environment, or a grant |
| `package` | The package id (`postgresql`) or the installation id | `network.tunnel` (MFA) on the node, or a grant |
| `ssh` | The node's label or id | `terminal.open` (MFA) on the node, or a grant |
| `raw` | The node's label or id, with `target.ports: [<port>]` | `network.tunnel.any` (MFA) on the node; never a grant |

`target.server_id` picks the node when the service runs on several;
`target.ports` keeps one remote port when the service publishes several.
When more than one (node, port) pair remains, the answer is
`409 ambiguous_target` with `choices` (catalogue entries), and the client
asks again with `server_id` and `ports`. Only nodes that are online count.

### Service port catalogue

What listens where, built from the stored app specs (`listen_port`,
container `ports`), the deployed compose stacks' published ports, installed
App Store packages (their `port` parameters, else their declared ports) and
each node's SSH endpoint (22 when none is set). Nothing is typed by hand.

| Method | Path | Authorization and behaviour |
| --- | --- | --- |
| GET | `/service-ports?service=&kind=&environment=` | `view` on the workspace — an array of entries on nodes the caller may view. `service` matches the id, name, `<environment>/<name>` or `<stack>/<service>`; `kind` is `app`, `stack`, `package` or `ssh`; `environment` a name or id |
| GET | `/servers/{id}/service-ports` | `view` on the node — `{server_id, ports: [entries]}` |

**Entry fields:** `kind`, `id` (the reference tunnels and grants store: the
service id, `<stack id>/<service>`, the installation id or the server
id), `name` (the app name, `<stack>/<service>`, the package id or the node's
label), `stack` and `service` (stack services), `label`
(`web/site`, `web/seq-b/seq`, `postgresql on lab-b`, `ssh on lab-b`),
`environment_id` and `environment` (apps and stack services), `server_id`,
`server` (the node's label), `online`, `port`, `proto` (`tcp`, `udp`),
`role` (`http`, `db`, `ssh` or `tcp`: what a client speaks to it) and
`dial_ip` (the address the agent dials on the node: loopback, or the
address the port is bound to, such as the cluster network address).

### Tunnels

| Method | Path | Authorization and behaviour |
| --- | --- | --- |
| POST | `/tunnels` | A person (no connector) — `{target: {kind, ref, server_id?, service?, ports?}, bind?, lifetime_minutes?, never_expires?, direct?}`. `direct: true` (`forge tunnel open --direct`): connections go to the node's SSH with a certificate from `POST /tunnels/{id}/ssh-cert`; `409` when the node does not report `ssh_tunnel` (after one fresh facts collection) or has no SSH endpoint. Checks the target as in the table above; `403` and an audited refusal otherwise (`network.tunnel.open`, with the reason). The lifetime is `min(lifetime_minutes, the grant's expiry, [tunnels] max_lifetime_hours)`; it defaults to `[tunnels] default_lifetime_hours`; `never_expires: true` needs `[tunnels] allow_never_expiry` and `access.manage` (`403` otherwise). `409 tunnel_limit` at a limit, `409 ambiguous_target` with `choices`. → `201` with the tunnel. Hold its control socket within 30 s or it is dropped. Audited as `network.tunnel.open` with the target, `raw`, `grant` and `expires_at` |
| GET | `/tunnels/{id}/control` | Owner, bearer session (the CLI) — WebSocket. The server sends `{"type": "open", "tunnel": …}`, then `{"type": "closed", "reason": …}` when the tunnel ends (`closed`, `idle`, `expired`, `access_withdrawn`, `client_closed`). Send `{"type": "refresh", "token": "<access token>"}` after a token refresh. Closing the socket ends the tunnel. Every 15 s the server re-checks that the sign-in or API key still exists and that the role or grant that allowed the tunnel still holds. Audited as `network.tunnel.close` with `bytes_up`, `bytes_down`, `duration_ms`, `reason` |
| POST | `/tunnels/{id}/ssh-cert` | Owner, a tunnel opened with `direct: true` and held by its control socket — re-checks access like the 15 s re-check (`403`, audited as a `network.tunnel.direct` denial, when it no longer holds or the tunnel expired). → `{host, port, host_key_fingerprint, user, dial_ip, dial_port, key_openssh, cert_openssh}`: the node's SSH endpoint and pinned host key, the account (`forge-tunnel`), and a fresh Ed25519 key with a certificate valid for 60 s. Its only principal is `forge.<server_id>.forge-tunnel`, its only extension `permit-port-forwarding`, and its key id `forge-tunnel/<tunnel>/<dial_ip>/<port>`, from which the node allows forwarding to that one destination. One certificate per connection; the key is never stored. Audited as `network.tunnel.direct`. The control socket also accepts `{"type": "active"}` from the CLI while direct connections are open, so the idle timeout holds |
| GET | `/tunnels/{id}/connect` | Owner — WebSocket for one TCP connection: binary frames both ways (at most 256 KiB each). Each direction ends on its own: send `{"type": "eof"}` when your side has no more data; `{"type": "eof"}` from the server means the node's side finished (half-close, like `shutdown(SHUT_WR)`). The socket closes when both have ended. A text frame `{"type": "error", "message"}` says why the node refused. `409` at the per-node or server limit |
| GET | `/tunnels?state=&all=` | Your tunnels; `all=true` lists everyone's (`user.manage`). `state` is `open` (default), `closed` (history, newest first, at most 200 rows) or `all` → `{tunnels, settings}`, where `settings` is the `tunnels` object of `GET /settings` |
| DELETE | `/tunnels/{id}` | Owner or `user.manage` → `204`; every connection ends at once. Audited as `network.tunnel.revoke` |

**Settings object:** `GET /settings` and the lists carry
`tunnels: {allow_never_expiry, default_lifetime_minutes,
max_lifetime_minutes, max_per_user}` from the server's `[tunnels]` section,
so clients can offer only lifetimes the server accepts.

**Tunnel fields:** `id`, `status` (`open`, `closed`), `user_id`, `user`,
`server_id`, `server` (the node's label), `target` (`{kind, ref, name,
label, environment_id, service}`), `port`, `role`, `dial_ip`, `bind` (as the
CLI reported it), `grant_id` (when a grant allowed it), `started_at`,
`expires_at` (`null` when it never expires), `never_expires`,
`connections`, `bytes_up` (to the node), `bytes_down` (from the node), and
for closed tunnels `closed_at` and `close_reason` (`closed`, `idle`,
`expired`, `access_withdrawn`, `client_closed`, `unclaimed`,
`server_restart`). Times are epoch milliseconds. Tunnels open when the
server stops are listed as closed with `server_restart`.

### Tunnel grants

An explicit grant lets one person tunnel to one target without a role on
its environment. Grants need `access.manage` on the workspace (roles
**Admin**, **Platform admin**, MFA).

| Method | Path | Authorization and behaviour |
| --- | --- | --- |
| GET | `/tunnel-grants?all=&user=` | Signed in — your active grants; with `access.manage`, everyone's (`user=<id>` for one person). `all=true` includes expired and revoked grants → `{grants, allow_never_expiry, can_never_expire, settings}` |
| POST | `/tunnel-grants` | `access.manage` (MFA) — `{user_id, target: {kind, ref, server_id?, service?, ports?}, lifetime_minutes?, never_expires?, note?}`. `kind` is `app`, `stack`, `package` or `ssh` (`raw` answers `400`); the target must exist in the catalogue (`404`), on any node unless `server_id` is given; `ports` as an array, a comma-separated string or `"*"` (every port, the default); `409 ambiguous_target` when several services match. The lifetime follows the same rules as a tunnel's. → `201` with the grant. Audited as `network.tunnel.grant.create` |
| DELETE | `/tunnel-grants/{id}` | `access.manage` → `204`. Open tunnels the grant allowed close at once (`access_withdrawn`). Audited as `network.tunnel.grant.revoke` with `tunnels_closed` |

**Grant fields:** `id`, `workspace_id`, `user_id`, `user`, `target`
(`{kind, ref, name, server_id, ports}`; `ports` empty means every port),
`server_id`, `ports`, `created_by`, `created_at`, `expires_at` (`null` when
it never expires), `never_expires`, `revoked_at`, `note`, `status`
(`active`, `expired`, `revoked`).

## Alerts

Alert rules and alarms ([concept](/docs/concepts/alerts/)). Paths are under
`/api/v1`. `workspace` defaults to this deployment's workspace.

| Method | Path | Authorization and behaviour |
| --- | --- | --- |
| GET | `/alert-rules?workspace=<id>` | `alert.view` on the workspace — the rules, by name |
| POST | `/alert-rules` | `alert.manage` — body below → `201` with the rule (`revision` `1`). `409` when the workspace already has `[alerts] max_rules_per_workspace` rules. Audited as `alert.rule.create` |
| GET | `/alert-rules/{id}` | `alert.view` — the rule plus `condition`, a plain description such as `is offline` or `> 85 for 30 min` |
| PUT | `/alert-rules/{id}` | `alert.manage` — same body. A changed `subject`, `scope`, `spec` or `enabled` starts a new `revision`; firing alarms of the old one end on the next pass. Name, description and severity keep the revision. Audited as `alert.rule.update` |
| DELETE | `/alert-rules/{id}` | `alert.manage` → `204`. Its alarms stay; firing ones end ("the rule was deleted"). Audited as `alert.rule.delete` |
| GET | `/alert-subjects?workspace=<id>` | `alert.view` on the workspace — `{subjects, seen_health}`: every subject a rule can watch (`{subject, pattern, kind, states?, unit?, scopes, source, description}`; `kind` is `number` or `state`, `source` `agent`, `probe` or `control_plane`, `pattern` `true` for `health.<purpose>.{check}`) and the concrete `health.*` subjects observed on the workspace's services (sorted, at most 200). See the [subjects table](/docs/concepts/alerts/#subjects) |
| GET | `/alert-rule-templates?place=<place>` | `alert.view` on the workspace — the [rule templates](/docs/reference/alert-rule-templates/): `{id, place, name, description, severity, subject, scope, spec}`. `place` is `node`, `service`, `database`, `certificate` or `any` (default: all); another value answers `400` |
| GET | `/alarms?workspace=&active=&rule=&server=&limit=` | `alert.view` on the workspace, and `view` on each alarm's resource (others are left out): for an installation's alarm `view` on its node, for a certificate's `view` on the workspace. Newest first; `active=true` only firing; `limit` 1–500, default `100` |
| GET | `/alarms/{id}` | `alert.view` + `view` on the resource (else `404`) — the alarm plus `events[]` (`{at, kind, actor?, detail}`; kinds `fired`, `resolved`, `ended`, `acknowledged`, `unacknowledged`, `note`) |
| POST | `/alarms/{id}/acknowledge` | `alert.manage` — optional `{note}` (≤ 2000 characters). The alarm keeps its state. Audited as `alert.alarm.acknowledge` |
| POST | `/alarms/{id}/unacknowledge` | `alert.manage`. Audited as `alert.alarm.unacknowledge` |
| POST | `/alarms/{id}/notes` | `alert.manage` — `{note}` (1–2000 characters) → `204`. Audited as `alert.alarm.note` |

An alarm takes at most 500 acknowledgements, unacknowledgements and notes in
all; one more answers `409`.

**Rule body:**

| Field | Type | Rules |
| --- | --- | --- |
| `workspace_id` | id | Optional; fixed after creation |
| `name` | string | 1–120 characters, no control characters |
| `description` | string | Optional, ≤ 2000 characters |
| `enabled` | bool | Default `true` |
| `severity` | string | `info`, `warning`, `major` or `critical` |
| `scope` | object | `{"kind": <kind>, "ids": [...]}`; no `ids` or `[]`: every resource of that kind in the workspace. At most 500 ids. `servers` (node ids of the workspace), `services` (service ids), `installations` (package installation ids of the workspace, databases included) or `certificates` (uploaded ingress certificate ids of the workspace) |
| `subject` | string | A subject from [`GET /alert-subjects`](/docs/concepts/alerts/#subjects) that applies to the scope's kind, such as `node.agent.status`, `node.disk.max_used_percent`, `package.state` or `health.readiness.<check>`. An unknown subject answers `400` (with "did you mean" for a near miss), and so does one for another scope kind. An unknown `node.*` subject is accepted for a `servers` scope, for measurements of newer agents |
| `spec` | object | `{trigger, mode, recovery}` below |

**`spec`:**

| Part | Shape |
| --- | --- |
| `trigger` | `{"kind": "threshold", "cmp": "gt"\|"ge"\|"lt"\|"le"\|"eq"\|"ne", "value": <number>}`, `{"kind": "state_in", "states": [...]}` or `{"kind": "state_not_in", "states": [...]}` (1–32 states of 1–64 characters) |
| `mode` | `{"mode": "immediate"}`; `{"mode": "sustained", "for_ms": <1000–86400000>}`; `{"mode": "window_average", "window_ms", "min_coverage_pct": <1–100>}` (numbers only); `{"mode": "repeated", "count": <1–1000>, "window_ms", "min_spacing_ms"}`; `{"mode": "any_in_window", "window_ms"}` |
| `recovery` | `{"condition": <trigger shape> \| null, "hold_ms": <0–86400000>}`. Without a condition: the trigger no longer holds |

**Alarm fields:** `id`, `workspace_id`, `rule_id`, `rule_revision`,
`title` (the rule's name when it fired), `severity`, `fingerprint`,
`resource` (`{kind, id}`; `kind` is `server`, `service`, `installation` or `certificate`), `server?` (the node of a service instance or an installation), `subject`, `state` (`firing`,
`resolved`, `ended`), `fired_value` (`{"type": "state"|"num", "value"}`),
`fired_at`, `resolved_at?` (also the end time of an ended alarm),
`end_reason?`, `acknowledged_by?`, `acknowledged_at?`, `previous?` (the
series' previous alarm), `silence?` (only while the alarm fires and an
active [silence](#silences) covers it: `{silence_id, until, reason}`, where
`until` is that silence's end). Times are epoch milliseconds.

## Notifications

Notification channels, policies and delivery history
([concept](/docs/concepts/alerts/#notifications),
[guide](/docs/guides/alarm-notifications/)). Paths are under `/api/v1`.
`workspace` defaults to this deployment's workspace. Times are epoch
milliseconds. What Forge sends to a channel is in the
[notification payload reference](/docs/reference/notification-payload/).

A channel, policy or delivery addressed by its id answers `404` both when
the id is unknown and when you may not use it for the request (the refusal
is still audited). If your role allows the request and only the MFA step is
missing, the answer is `403` `step_up_required`.

| Method | Path | Authorization and behaviour |
| --- | --- | --- |
| GET | `/notification-channels?workspace=<id>` | `alert.view` on the workspace — the channels, by name |
| POST | `/notification-channels` | `notification.manage` (MFA) — channel body below → `201` with the channel. `422` (`invalid`) with the reason when the name, the URL, `secret_item` or an `email` field is refused; `409` when the workspace already has 50 channels. Audited as `notification.channel.create` |
| GET | `/notification-channels/{id}` | `alert.view` — the channel; `404` when there is none |
| PUT | `/notification-channels/{id}` | `notification.manage` (MFA) — same body, which replaces the channel. Leave `url` out (or send `""`) to keep the stored URL; leave `secret_item` out to stop signing; leave `email.password` out to keep the stored SMTP password (only while `smtp_host`, `smtp_port`, `tls` and `username` stay the same; otherwise `422`). A different `workspace_id` answers `400`. Audited as `notification.channel.update` with `url_changed` and `password_changed` |
| DELETE | `/notification-channels/{id}` | `notification.manage` (MFA) → `204`. `409` while a policy uses the channel; the message names the policies. Its delivery history stays. Audited as `notification.channel.delete` |
| POST | `/notification-channels/{id}/test` | `notification.manage` (MFA) — no body. Sends one test message now, with one attempt and no retry → `200` with the delivery and its `attempts[]`, whether the receiver accepted it or not: read `status`. `409` when the channel is turned off. `429` (`rate_limited`) when the last test to this channel is less than 10 seconds old. Every outcome is audited as `notification.channel.test`, the refusals too |
| GET | `/notification-policies?workspace=<id>` | `alert.view` on the workspace — the policies, by name |
| POST | `/notification-policies` | `alert.manage` — policy body below → `201` with the policy. `422` (`invalid`) when a field is refused; `409` when the workspace already has 100 policies. Audited as `notification.policy.create` |
| GET | `/notification-policies/{id}` | `alert.view` — the policy; `404` when there is none |
| PUT | `/notification-policies/{id}` | `alert.manage` — same body, which replaces the policy. A different `workspace_id` answers `400`. Audited as `notification.policy.update` |
| DELETE | `/notification-policies/{id}` | `alert.manage` → `204`. Deliveries already planned are still sent. Audited as `notification.policy.delete` |
| GET | `/notification-deliveries?workspace=&alarm=&channel=&status=&limit=` | `alert.view` on the workspace, and `view` on each delivery's alarm resource (others are left out; test messages are always listed). Newest first. `alarm` and `channel` are ids; `status` is `pending`, `sending`, `sent`, `failed`, `cancelled` or `suppressed`; `limit` 1–500, default `100` |
| GET | `/notification-deliveries/{id}` | `alert.view`, then `view` on the alarm's resource (`404` without either) — the delivery plus `attempts[]`, oldest first |

Unknown body fields are refused. A denied request is audited by the
authorization layer under the action that was checked. Sending a message is
not audited: the delivery history is the record. Audit details hold ids,
names, the kind and the target host, and for an email channel its `email`
settings with `password_set`; never the URL, the SMTP password or another
secret.

**Channel body:**

| Field | Type | Default | Rules |
| --- | --- | --- | --- |
| `workspace_id` | id | This deployment's workspace | Fixed after creation |
| `name` | string | Required | 1–80 characters, no control characters |
| `kind` | string | Required | `webhook` (Forge's JSON payload, optionally signed), `slack` (`{"text": …}` for Slack-compatible incoming webhooks) or `email` (a plain-text [email over SMTP](/docs/reference/notification-payload/#email)) |
| `enabled` | bool | `true` | A channel that is turned off gets no new deliveries; waiting ones are cancelled |
| `url` | string | Required on create for `webhook` and `slack`; refused for `email` | `https://host[:port]/path[?query]`, at most 2000 bytes. Write-only: no answer returns it. See the URL rules below. Turning an `email` channel into another kind needs a new `url` |
| `secret_item` | credential vault item id | None | `webhook` only (`422` for `slack`). The item's password is the HMAC-SHA256 key of the [signature](/docs/reference/notification-payload/#signature). It must be at least 16 bytes (`422` otherwise). Setting it needs the right to use the item: **Platform admin**, a browser session (an API key answers `403`) and `secret.read` (MFA); the item must exist (`404`). Checked on create, and on update when the item changes or a new `url` is given |
| `email` | object | Required for `email`; refused for the other kinds | The SMTP settings below |

**Email settings** (`email` object). The answer returns the same object
without `password` and with `password_set` (bool). Unknown fields are
refused.

| Field | Type | Default | Rules |
| --- | --- | --- | --- |
| `smtp_host` | string | Required | Host name or IP address, at most 253 bytes. The URL rules below apply to the host and, when a message is sent, to every address it resolves to |
| `smtp_port` | integer | `587` for `starttls`, `465` for `tls`, `25` for `none` | 1–65535 |
| `tls` | string | `starttls` | `starttls`: STARTTLS is required; a server that does not offer it is not sent to. `tls`: TLS from the first byte (SMTPS). `none`: unencrypted, only when `[alerts] webhook_allow_http` is `true`, and never with `username`. The certificate is verified against `smtp_host` |
| `username` | string | None | At most 256 bytes. Without it Forge sends without authentication and keeps no password |
| `password` | string | Required with a new `username`, and when `smtp_host`, `smtp_port` or `tls` changes | 1–1024 bytes, one line. Write-only: encrypted at rest, never returned, never logged. Leave it out of an update to keep the stored password; it is kept only for the same host, port, TLS mode and user, so it is never sent to a server it was not set for |
| `from` | string | Required | `forge@example.com` or `Forge <forge@example.com>` |
| `to` | array of strings | Required | 1–20 addresses, same forms as `from`; duplicates are dropped |
| `subject_prefix` | string | None | At most 40 characters, one line. Put before every subject, for example `[prod]` |

```json
{
  "name": "Ops mail",
  "kind": "email",
  "email": {
    "smtp_host": "smtp.example.com",
    "tls": "starttls",
    "username": "forge@example.com",
    "password": "<smtp-password>",
    "from": "Forge <forge@example.com>",
    "to": ["ops@example.com", "oncall@example.com"],
    "subject_prefix": "[prod]"
  }
}
```

An email channel's `target` is `smtp://<host>:<port>`, or
`smtps://<host>:<port>` with `tls`.

**URL rules** (`422` with the reason when saving; the same rules run on
every address the host name resolves to when a message is sent, where a
refusal fails the attempt as `blocked_target`):

| Rule | Setting that relaxes it |
| --- | --- |
| The scheme is `https` | `[alerts] webhook_allow_http` also allows `http` |
| No user name, password or fragment in the URL | None |
| IP addresses in normal form; numeric spellings such as `2852039166` or `0x7f.1` are refused | None |
| No link-local, cloud metadata, multicast, broadcast, unspecified, reserved or documentation address, and no metadata host name (`metadata.google.internal`, `instance-data`, …) | None |
| No IPv6 address that stands for an IPv4 destination through a relay or translator: 6to4 (`2002::/16`), Teredo (`2001::/32`), local-use NAT64 (`64:ff9b:1::/48`), IPv4-translated (`::ffff:0:0:0/96`) | None |
| No loopback address, `localhost` or `*.localhost` | `[alerts] webhook_allow_loopback` |
| No private network address (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `100.64.0.0/10`, `198.18.0.0/15`, `fc00::/7`, `fec0::/10`) | `[alerts] webhook_allow_private` |

**Channel fields:** `id`, `workspace_id`, `name`, `kind`, `enabled`,
`target` (the URL's `scheme://host[:port]` only), `secret_item?`,
`created_by` (`user:<id>`), `created_at`, `updated_at`. The URL is never
returned.

```json
{
  "id": "<channel-id>",
  "workspace_id": "<workspace-id>",
  "name": "Ops webhook",
  "kind": "webhook",
  "enabled": true,
  "target": "https://hooks.example.com",
  "secret_item": "<vault-item-id>",
  "created_by": "user:<user-id>",
  "created_at": 1731750000000,
  "updated_at": 1731750000000
}
```

**Policy body:**

| Field | Type | Default | Rules |
| --- | --- | --- | --- |
| `workspace_id` | id | This deployment's workspace | Fixed after creation |
| `name` | string | Required | 1–80 characters, no control characters |
| `enabled` | bool | `true` | A policy that is turned off matches no alarm |
| `min_severity` | string | Required | `info`, `warning`, `major` or `critical`: alarms of this severity and worse |
| `rules` | list of alert rule ids | `[]` | Empty: alarms of every rule. At most 100; each must be a rule of the workspace |
| `scope` | object | Absent: every resource | `{"kind": "servers", "ids": [<server-id>…]}` or `{"kind": "services", "ids": […]}`. Empty `ids`: every resource of that kind. At most 500 ids |
| `events` | list of strings | Required | At least one of `fired` and `resolved`. `resolved` also covers alarms that ended without recovery |
| `repeat_interval_secs` | integer, seconds | Absent: no reminders | 300–604800 (5 minutes to 7 days). Needs `fired` in `events`. Reminders go out while the alarm fires and is not acknowledged, at most 50 per alarm and channel |
| `channels` | list of channel ids | `[]` | Up to 10 channels of the workspace. At least one unless `email_users` or `email_roles` is set |
| `email_channel` | channel id | None | The `email` channel of the workspace that sends to `email_users` and `email_roles` (`422` for another kind). Required when either is set; dropped when both are empty. Its own `to` addresses are not added |
| `email_users` | list of user ids | `[]` | Up to 100 users, each with a role on the platform, this workspace or something in it (`422` otherwise). A user is emailed only while they hold `alert.view` on the alarm's resource, the account is enabled and has an email address |
| `email_roles` | list of role names | `[]` | Up to 20 of `platform_admin`, `workspace_admin`, `operator`, `developer`, `dba`, `auditor`, `customer`. Everyone who holds the role on the alarm's resource or on any of its ancestors (environment, cluster, workspace, platform) is emailed, with the same checks as `email_users`. A binding on a sibling resource does not count |

```json
{
  "name": "Page operators",
  "min_severity": "critical",
  "events": ["fired", "resolved"],
  "channels": [],
  "email_channel": "<channel-id>",
  "email_users": ["<user-id>"],
  "email_roles": ["operator"]
}
```

**Policy fields:** the body's fields plus `id`, `created_by`, `created_at`,
`updated_at` and `warnings`. `scope`, `repeat_interval_secs` and
`email_channel` are left out when unset. `warnings.users_without_email`
lists the `email_users` that have no email address (they are skipped until
they set one); it is left out when there are none. A channel reached by two
matching policies gets one message per alarm event. Policy audit details
hold `email_channel`, `email_users` (ids) and `email_roles`, never
addresses.

**Delivery fields:**

| Field | Type | Present | Meaning |
| --- | --- | --- | --- |
| `id` | id | Always | The delivery; sent as `X-Forge-Delivery` |
| `workspace_id` | id | Always | |
| `alarm_id` | id | Not for a test | The alarm |
| `event` | string | Always | `fired`, `resolved`, `ended`, `reminder` or `test` |
| `occurrence` | integer | Always | `0`, or the number of the reminder |
| `policy_id` | id | Not for a test | The policy that planned it |
| `channel_id` | id | Always | |
| `status` | string | Always | `pending`, `sending`, `sent`, `failed`, `cancelled` or `suppressed` ([meanings](/docs/reference/notification-payload/#delivery-statuses)) |
| `attempt_count` | integer | Always | Attempts started so far |
| `next_attempt_at` | integer, epoch ms | Always | When the next attempt is due, while `pending` |
| `last_error` | string | After a failed attempt | `timeout`, `connect`, `tls`, `blocked_target`, `http_status`, `secret` or `config` ([classes](/docs/reference/notification-payload/#error-classes)) |
| `last_http_status` | integer | When the receiver answered | The last status code |
| `detail` | string | When `cancelled` or `suppressed` | [Why it was cancelled](/docs/reference/notification-payload/#cancel-reasons); for `suppressed`, `silenced: <reason>` |
| `silence_id` | id | When `suppressed` or `after_silence` | The silence that held it back, or the one this message follows |
| `after_silence` | boolean | Only when `true` | Sent because a silence ended; it stands for the messages that silence held back |
| `created_at` | integer, epoch ms | Always | When it was planned |
| `finished_at` | integer, epoch ms | When final | When it became `sent`, `failed`, `cancelled` or `suppressed` |
| `channel_name`, `policy_name`, `alarm_title` | string | Always | Names when the delivery was planned. `policy_name` and `alarm_title` are empty for a test |
| `resource` | `{kind, id}` | Not for a test | The alarm's resource |
| `recipients` | object | On a policy's delivery to people (`email_users`, `email_roles`) | Whom the message goes to, fixed when it was planned: retries and the message after a silence use the same addresses. `count` (integer): the number of addresses. `to` (list of strings): the addresses, only for callers with `notification.manage` on the workspace (left out for others). `skipped` (list of `{user_id, reason}`): people left out, with `reason` `no_email`, `no_access`, `disabled` or `too_many` (more than 200 addresses); never their addresses. A delivery whose people resolve to nobody is `cancelled` with `detail` `no recipients` |

```json
{
  "id": "<delivery-id>",
  "status": "sent",
  "event": "fired",
  "channel_name": "Ops mail",
  "policy_name": "Page operators",
  "recipients": {
    "count": 2,
    "to": ["ana@example.com", "op@example.com"],
    "skipped": [{ "user_id": "<user-id>", "reason": "no_email" }]
  }
}
```

The answers of `GET /notification-deliveries/{id}` and of the test
endpoint add `attempts`, the list of attempts, oldest first. Each attempt is
`{delivery_id, number, at, ok, http_status?, error?, duration_ms}`:
`number` starts at `1`, `error` is an error class, `duration_ms` is how
long the attempt took.

Final deliveries are kept for 30 days.

## Silences

Silences and maintenance windows
([concept](/docs/concepts/silences/), [guide](/docs/guides/silence-alarms/)). A
silence holds back the notifications of the alarms it covers; it never
changes evaluation. Paths are under `/api/v1`. `workspace` defaults to this
deployment's workspace. Times are epoch milliseconds.

| Method | Path | Authorization and behaviour |
| --- | --- | --- |
| GET | `/silences?workspace=&state=&limit=` | `alert.view` on the workspace lists every silence; otherwise only silences with `alert.view` on everything their scope names. Latest start first. `state` is `scheduled`, `active` or `expired`; `limit` 1–500, default `100`. An empty answer needs `alert.view` on the workspace, else `403` |
| POST | `/silences` | `alert.manage` on everything the scope names (`403` naming the entity otherwise, also when the entity does not exist) — body below → `201` with the silence. `422` (`invalid`) when a field is refused; `400` when both or neither of `ends_at` and `duration_secs` are given, or for another workspace; `409` when you already have 50 silences that have not ended, or the workspace has 200 (workspace managers are exempt from the workspace cap). Audited as `alert.silence.create` |
| GET | `/silences/{id}` | `alert.view` on the workspace or on everything the scope names — the silence. `404` when it does not exist or you may not read it |
| POST | `/silences/{id}/extend` | Readable (else `404`), then `alert.manage` on everything the scope names — `{"ends_at": <ms>}` or `{"extend_secs": <n>}`, exactly one (`400` otherwise). The new end must be later than the current end (`422`) and at most 30 days after the start (`422`). `409` when the silence has ended. Audited as `alert.silence.extend` with `previous_ends_at` |
| POST | `/silences/{id}/end` | Readable (else `404`), then `alert.manage` on everything the scope names — no body. Sets `ends_at` to now and `ended_by` to you; a scheduled silence never takes effect. Covered alarms that still fire notify within seconds. `409` when it has already ended. Audited as `alert.silence.end` with `previous_starts_at` and `previous_ends_at` |

Unknown body fields are refused. Audit details hold the silence id, scope,
rules, severities, window and reason.

**Silence body (`POST /silences`):**

| Field | Type | Default | Rules |
| --- | --- | --- | --- |
| `workspace_id` | id | This deployment's workspace | Fixed after creation |
| `scope` | object | Required | `{"kind": "workspace"}`, `{"kind": "cluster", "id": <cluster-id>}`, `{"kind": "environment", "id": <environment-id>}`, `{"kind": "servers", "ids": [<server-id>…]}` or `{"kind": "services", "ids": [<service-id>…]}`. `ids` holds 1–100 ids. Every entity must exist in the workspace (`422` "scope: no …") |
| `rules` | list of alert rule ids | `[]` | Empty: alarms of every rule. At most 100, each a rule of the workspace. Duplicates are dropped |
| `min_severity` | string | None | `info`, `warning`, `major` or `critical`: only alarms of this severity and worse |
| `max_severity` | string | None | Only alarms of this severity and milder. A worse alarm still notifies. Must not be milder than `min_severity` |
| `starts_at` | integer, epoch ms | Now | A later time makes a maintenance window. At most 365 days ahead. Up to 5 minutes in the past counts as now; older is refused |
| `ends_at` | integer, epoch ms | — | Exclusive end. Give this or `duration_secs` |
| `duration_secs` | integer, seconds | — | Length from the start: 60–2592000 (1 minute to 30 days). Give this or `ends_at` |
| `reason` | string | Required | 1–500 characters after trimming; no control characters except line breaks |

The end must be in the future, and the silence must last 1 minute to 30
days from its start.

**Silence fields:**

| Field | Type | Present | Meaning |
| --- | --- | --- | --- |
| `id` | id | Always | The silence |
| `workspace_id` | id | Always | |
| `scope` | object | Always | As in the body |
| `rules` | list of ids | Always | `[]`: every rule |
| `min_severity`, `max_severity` | string | When set | Severity bounds |
| `starts_at` | integer, epoch ms | Always | When it starts holding |
| `ends_at` | integer, epoch ms | Always | Exclusive end. Ending early sets it to the time it was ended |
| `reason` | string | Always | |
| `created_by` | principal | Always | `user:<id>` |
| `created_at`, `updated_at` | integer, epoch ms | Always | |
| `ended_by` | principal | When ended early | Who ended it |
| `state` | string | Always | `scheduled`, `active` or `expired`, by the server's clock |

```json
{
  "id": "<silence-id>",
  "workspace_id": "<workspace-id>",
  "scope": { "kind": "cluster", "id": "<cluster-id>" },
  "rules": [],
  "max_severity": "major",
  "starts_at": 1764626400000,
  "ends_at": 1764640800000,
  "reason": "Kernel upgrade, change <change-id>",
  "created_by": "user:<user-id>",
  "created_at": 1764500000000,
  "updated_at": 1764500000000,
  "state": "scheduled"
}
```

Silences that ended more than 30 days ago, and that no kept delivery names,
are deleted.

## Service logs

Structured events that services send with the Seq protocol, stored in the
analytics database. See [Send and search service logs](/docs/guides/application-logs/).

| Method | Path | Permission |
| --- | --- | --- |
| POST | `/api/events/raw` (also `/api/events/raw?clef`) | The `X-Seq-ApiKey` ingest key; no session. Not under `/api/v1`. Body below → `201` `{"MinimumLevelAccepted":null}` |
| GET | `/logs?services=&instances=&node=&container=&stack=&source=&levels=&q=&from=&to=&before=&after=&limit=` | `log.read` on each named service (`404` when one is unknown or not readable); without a service selection, the services on which the caller holds it, plus the [container lines](/docs/concepts/container-logs/) that belong to no service on nodes where the caller holds `view` → `{events, next, newest}` |
| GET | `/logs/histogram?services=&instances=&node=&container=&stack=&source=&levels=&q=&from=&to=` | `log.read`, as for `/logs` → `{from, to, step_ms, buckets}` |
| GET | `/services/{id}/log-keys` | `log.manage` on the service → array of key objects |
| POST | `/services/{id}/log-keys` | `log.manage` on the service — `{name}` (1 to 80 characters, no control characters) → `201` `{key, token}`. Audited as `service.log_key.create` |
| DELETE | `/log-keys/{id}` | `log.manage` on the key's service → `204`. Audited as `service.log_key.revoke` |

### Ingest body

The body is either newline-delimited CLEF objects, or one JSON object
`{"Events":[...]}` (the older Seq raw format). Forge detects the format; the
`?clef` query string is accepted and ignored.

| CLEF field | Raw-format field | Meaning |
| --- | --- | --- |
| `@t` | `Timestamp` | Event time, RFC 3339. Missing or unreadable: the ingest time |
| `@l` | `Level` | Level. Missing or unknown: `Information` |
| `@mt` | `MessageTemplate` | Message template, for example `Order {OrderId} placed` |
| `@m` | `RenderedMessage` | Rendered message. Missing: rendered from `@mt` and the properties |
| `@x` | `Exception` | Exception text |
| `@i` | | Event type, a number or hex string. Missing: a hash of the template |
| `@tr`, `@sp` | | Trace id and span id |
| `@r` | | Ignored: Forge renders the message itself |
| other keys | `Properties` | Properties, as sent. `@@name` stores a property named `@name`; other `@` keys are dropped |

Levels accept these names in any case:

| Level | Accepted names |
| --- | --- |
| `Verbose` | `verbose`, `trace`, `vrb`, `trc` |
| `Debug` | `debug`, `dbg` |
| `Information` | `information`, `info`, `inf`, `notice` |
| `Warning` | `warning`, `warn`, `wrn` |
| `Error` | `error`, `err`, `eror`, `fail` |
| `Fatal` | `fatal`, `critical`, `crit`, `ftl`, `alert`, `emergency`, `emerg` |

Limits: at most 10,000 events per request and 16 MiB per body. A message,
template or exception longer than 32 KiB is cut and ends in `…`; when the
property bag is larger than 32 KiB, values longer than 1 KiB are cut to
1 KiB.

### Query parameters

| Parameter | Type | Default | Meaning |
| --- | --- | --- | --- |
| `services` | comma-separated ids | none | Whole services: every event of each, whichever node sent it (also events with no known node). At most 200 |
| `instances` | comma-separated `<service id>:<node id>` | none | Single nodes of a service: only events Forge attributed to that node. At most 200. OR'ed with `services`; with neither, every service the caller may read |
| `service` | id | none | One service; the same as `services=<id>` |
| `node` | comma-separated node ids | none | Only events from these nodes |
| `container` | string, at most 256 characters | none | Container lines of this container: its name, or a prefix of its id |
| `stack` | string, at most 256 characters | none | Lines of this stack: `<environment>/<stack>`, or a compose project |
| `source` | `ingest` or `container` | both | `ingest`: sent by services with an ingest key. `container`: collected from Docker (`400` otherwise) |
| `levels` | comma-separated level names | every level | For example `Warning,Error`; the names in the table above |
| `q` | string | none | Search: `Name="value"` or `Name=value` match a property exactly (at most 8; names are letters, digits, `_`, `.`, `-`); the rest is free text matched against message and exception, ignoring case, at most 500 characters |
| `from`, `to` | epoch ms | none (`/logs`); the last 24 hours (`/logs/histogram`) | Time window on the event time `ts` |
| `before` | cursor | none | Older events than this cursor: the `next` of a previous page |
| `after` | cursor | none | Newer events than this cursor: the `newest` of a previous page (live tail). Not with `before` |
| `limit` | integer | `100` | Events per page, clamped to 1 to 500. `/logs` only |

`/logs` returns events newest first. `next` is `null` at the end of the log.
With `after`, an empty page keeps `newest` equal to the cursor passed.

The histogram window may be at most 31 days. Its step is the smallest of
1 s, 5 s, 10 s, 30 s, 1 min, 5 min, 10 min, 30 min, 1 h, 3 h, 6 h, 12 h,
1 day and 7 days that gives at most about 60 buckets. Each bucket is
`{t, counts}`, where `counts` maps a level to a number; empty buckets are
included.

### Event object

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | UUID | UUIDv7 assigned at ingest |
| `service_id`, `service` | id or `null`, string or `null` | The service and its name; `null` for a container line that belongs to no service |
| `ts` | epoch ms | When the app says the event happened |
| `node_ts` | epoch ms or `null` | When a node's agent received it; reserved for agent ingest, `null` today |
| `ingest_ts` | epoch ms | When Forge stored it; retention counts from here |
| `level` | string | `Verbose`, `Debug`, `Information`, `Warning`, `Error` or `Fatal` |
| `message`, `message_template` | string | Rendered message and template |
| `exception`, `trace_id`, `span_id` | string or `null` | As sent |
| `event_type` | string | Eight hex digits |
| `server_id`, `node` | id, string, or `null` | The node that sent the event and its console name; `null` when Forge could not tell (see below) |
| `properties` | object | Every other property |
| `source` | string | `ingest` or `container` |
| `container_id`, `container_name`, `stack`, `stream` | string or `null` | Container lines: the short container id, its name, its stack and `stdout` or `stderr`; `null` for ingested events |

**Node of an event.** At ingest Forge reads the first of the properties
`MachineName`, `Node`, `Host`, `HostName` and `hostname` that names one of
the nodes the service is placed on. A name matches the node's hostname, its
first label (`web-1` for `web-1.example.com`) or its console name, ignoring
case. Only the service's own nodes can match, so a key cannot attach events
to another node. Events without such a property, from a node the service is
not placed on, or stored before node attribution existed have `null`. A
placement change is picked up within 30 seconds. Serilog sends
`MachineName` with `Enrich.WithMachineName()` (`Serilog.Enrichers.Environment`).

### Key object

| Field | Type | Meaning |
| --- | --- | --- |
| `id`, `service_id` | id | The key and its service |
| `name` | string | Where the key is used |
| `prefix` | string | First 10 characters of the token, to tell keys apart |
| `created_at`, `last_used_at` | epoch ms | `last_used_at` is updated at most once a minute; `null` until first use |
| `created_by` | id or `null` | The user who created it |

`token` (in the create response only) starts with `flk_`. Forge stores only
its SHA-256 hash and cannot return it again.

### Errors

| Status | Code | When |
| --- | --- | --- |
| `401` | `invalid_api_key` | Ingest without a key, or with an unknown or revoked key |
| `400` | | A line that is not a JSON object, more than 10,000 events, an invalid cursor or level, `before` with `after`, a histogram window that is empty or longer than 31 days, or an invalid key name |
| `403` | | The caller lacks `log.read` or `log.manage` on the named service |
| `404` | | Unknown service or key |
| `503` | `analytics_off` | `[analytics] engine = "off"` |
| `503` | `analytics_worker_down` | The analytics worker process is restarting; clients should retry |
| `503` | `analytics_unavailable` | The analytics database cannot be reached, or the worker did not answer in time; clients should retry |
| `503` | `ingest_busy` | Ingest: more than 4,096 requests, or more than `ingest_max_queued_mb` of parsed events, are waiting to be stored; clients should retry shortly |
| `503` | `ingest_disk_low` | Ingest: the analytics engine has less free disk than `min_free_disk_mb`; clients should retry later |
| `503` | `search_busy` | `/logs` or `/logs/histogram`: `search_max_concurrent` searches are already running; retry shortly |

**How ingest stores events.** `forge-server` checks the key and passes the
body to the analytics worker, which parses it. Forge does not run one
insert per request. The worker queues each request's parsed events for one
writer, which takes everything
queued, waits up to `ingest_flush_interval_ms` (default 100 ms) for more, and
stores it in one insert (at most `ingest_max_batch_rows`, default 20,000
rows). The request is answered after its events are stored, so `201`
means stored; a failed insert answers every waiting request with `503`
`analytics_unavailable`. A request therefore takes up to about one flush
interval longer than the insert itself. A request still waiting after 30
seconds fails with `503`; if its insert had already started, the retry
can store its events a second time (delivery is at least once).

**Search limits.** `/logs` and `/logs/histogram` run under the
[analytics settings](#analytics-settings): at most `search_max_concurrent`
at once, each stopped after `search_timeout_secs`, with `search_threads` and
`search_memory_mb` passed to the engine. When `search_max_lookback_days` is
above `0`, a `from` earlier than that many days ago (or no `from`) is moved
forward to the limit, without an error.

## Jobs

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/jobs?workspace=&status=&limit=` | `view` (filtered); `limit` default `100` |
| GET | `/jobs/{id}`, `/jobs/{id}/steps` | `view` |
| GET | `/jobs/{id}/live` | `view` — `{output, status}`: output the node streams while the job's command runs; `output` is `null` when nothing is streaming |
| POST | `/jobs` | `job.create` + handler action — direct kinds only (`diagnostics.sleep`) → `202` job |
| POST | `/jobs/{id}/cancel` | `job.cancel` → `204`; `409` when the job already finished. Audited as `job.cancel` |

Cancelling a queued or running job stops its running step at once. When the
job has an operation running on a node (a recipe, an install), Forge also
sends that node a signed `cancel` command for the operation. The agent stops
the operation and the helper kills its process group. Sending the cancel is
best effort: if it cannot be queued, Forge logs a warning and the job is
still cancelled. A node whose `agent.toml` does not allow `cancel` refuses it
and the operation runs to the end (see
[Configuration](/docs/reference/configuration/#agent-agenttoml)).

## Audit

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/audit?workspace=&limit=&q=&action=&actor=&outcome=&resource=&since=&until=&before=` | `audit.read` on the workspace's audit log, or on platform without `workspace` |
| GET | `/audit/page?…&total=` | `audit.read`, as for `/audit` |
| GET | `/audit/histogram?…&buckets=` | `audit.read`, as for `/audit` |
| GET | `/audit/actions?workspace=` | `audit.read`, as for `/audit` |
| GET | `/audit/verify` | `audit.read` on platform |

`GET /audit` returns a JSON array of entries, newest first.
`GET /audit/page` takes the same filters and returns one page with a cursor
for the next one. Both filter in the database, so every entry is reachable,
however old. All query parameters are optional:

| Parameter | Type | Default | Notes |
| --- | --- | --- | --- |
| `workspace` | string | none | Only this workspace's entries |
| `limit` | integer | `100` on `/audit`, `50` on `/audit/page` | Most entries returned. Capped at `500` on `/audit` and `200` on `/audit/page`; values below `1` count as `1` |
| `q` | string | none | Free text: entries whose action, actor, resource or details contain this, case-insensitive. Combines with the other filters |
| `action` | string | none | Action prefixes, such as `stack.` or `auth.login`. Comma-separated (`action=auth.,file.`) or repeated (`action=auth.&action=file.`); an entry matches if its action starts with any of them. Up to 50 prefixes. Case-sensitive on PostgreSQL |
| `actor` | string | none | Actors that contain this, case-insensitive, such as `user:<id>` or `system:forge` |
| `outcome` | string | none | `success`, `denied` or `failure`, case-insensitive. Aliases: `allow`, `ok` → `success`; `deny` → `denied`; `error`, `failed` → `failure`. Any other value matches nothing |
| `resource` | string | none | Resources that contain this, case-insensitive, such as `server:<id>` |
| `since` | integer | none | Epoch ms; only entries at or after it (inclusive) |
| `until` | integer | none | Epoch ms; only entries before it (exclusive). `400` when not after `since` |
| `before` | integer | none | Cursor: only entries with a `seq` below this. Pass the previous page's `next_cursor` |
| `total` | boolean | `false` | `/audit/page` only: also count every matching entry |
| `buckets` | integer | `60` | `/audit/histogram` only: most buckets. Clamped to `1`–`200` |

`%` and `_` in text filters match themselves, not wildcards.

`GET /audit/page` response:

| Field | Type | Notes |
| --- | --- | --- |
| `entries` | array | Entries, newest first, in the [entry format](/docs/concepts/audit/#entry-format) |
| `next_cursor` | integer or `null` | The `seq` to pass as `before` for the next, older page; `null` on the last page |
| `total` | integer | Matching entries across all pages, ignoring `before`. Present only with `total=true` |

```sh
curl -s -H "Authorization: Bearer $FORGE_TOKEN" \
  "https://<forge-host>/api/v1/audit/page?action=auth.,file.&outcome=denied&since=1767225600000&limit=50&total=true"
```

```json
{
  "entries": [{ "seq": 4182, "action": "auth.login", "outcome": "denied", "…": "…" }],
  "next_cursor": 4120,
  "total": 73
}
```

Fetch the next page with `before=4120` and the same filters. New entries
written in the meantime don't shift the pages, because the cursor is a
sequence number, not an offset.

`GET /audit/histogram` takes the same filters (`before` and `limit` are
ignored) and counts the matching entries per time bucket and outcome. The
window runs from `since`, or the first entry in the log, to `until`, or now.
Buckets have a round width (1 s, 5 s, 1 min, 30 min, 6 h, 1 day…) chosen so
the window fits in at most `buckets` of them, plus one when the start is not
on a bucket edge. Every bucket is listed, empty ones included. An unknown
`outcome` returns the same window with every count at `0`.

| Field | Type | Notes |
| --- | --- | --- |
| `from` | integer | Start of the window, epoch ms |
| `to` | integer | End of the window, epoch ms (exclusive) |
| `step_ms` | integer | Width of every bucket, in ms |
| `buckets` | array | Oldest first: `{t, success, denied, failure}`, where `t` is the bucket start (epoch ms) and the others are entry counts |

```sh
curl -s -H "Authorization: Bearer $FORGE_TOKEN" \
  "https://<forge-host>/api/v1/audit/histogram?since=1767225600000&q=alice&buckets=48"
```

```json
{
  "from": 1767225600000,
  "to": 1767312000000,
  "step_ms": 1800000,
  "buckets": [{ "t": 1767225600000, "success": 12, "denied": 1, "failure": 0 }, "…"]
}
```

`GET /audit/actions` returns every distinct action in the log with its count,
sorted by action:

```json
[
  { "action": "auth.login", "count": 412 },
  { "action": "cluster.create", "count": 3 }
]
```

## Plugins

| Method | Path | Permission |
| --- | --- | --- |
| GET | `/plugins` | `view` on platform |
| POST | `/plugins` | `plugin.manage` (MFA) — install |
| POST | `/plugins/upgrade` | `plugin.manage` (MFA) |
| POST | `/plugins/{id}/enable` · `disable` · `pin` · `unpin` | `plugin.manage` (MFA) |
| DELETE | `/plugins/{id}` | `plugin.manage` (MFA) |
| POST | `/plugins/{id}/invoke` | `plugin.invoke` on `scope` — `{scope?, input}` |

## Live events

`GET /events` upgrades to a WebSocket. Messages:

```json
{"type": "job", "job_id": "…", "kind": "agent.command", "status": "running", "step": "dispatch"}
{"type": "server", "server_id": "…", "status": "online"}
```

Each event is authorized for the subscriber (`view`) before it is sent. The
`Origin` header must match `public_url`.
