---
title: Single sign-on (OIDC)
description: Connect Forge to Entra ID, Okta, Google, Keycloak or any OpenID Connect provider.
sidebar:
  order: 4
---

This guide shows you how to let users sign in to Forge with an OpenID
Connect provider such as Entra ID, Okta, Google or Keycloak.

## Before you begin

- You need shell access to the Forge server to edit `forge.toml` and restart
  the service, and an administrator account at your identity provider.
- `public_url` in `forge.toml` must be the URL users open the console at.
- The server needs outbound HTTPS to your provider and the system CA bundle
  (see [Platforms](/docs/reference/platforms/)).

## Steps

1. At your provider, register Forge as an application:
   - Application type: web (confidential client).
   - Redirect URI: `<public_url>/api/v1/auth/oidc/callback`.
   - Scopes: `openid email profile`.
2. Add the provider to `forge.toml`:

   ```toml
   [oidc]
   issuer_url = "https://login.example.com/realms/acme"
   client_id = "orbyn-forge"
   client_secret_env = "FORGE_OIDC_CLIENT_SECRET"
   auto_provision = false
   ```

3. Put the client secret in the environment variable named by
   `client_secret_env`, never in the file. With systemd, add it to the
   service with `sudo systemctl edit forge-server`:

   ```ini
   [Service]
   Environment=FORGE_OIDC_CLIENT_SECRET=<client-secret>
   ```

4. Restart Forge. It discovers the provider's metadata at start-up.

## How users are matched

1. By the provider's subject (`iss|sub`), if previously linked.
2. Otherwise by **verified** email, and the subject is linked.
3. Otherwise, only if `auto_provision = true`, a new user is created with no
   roles.

Create the users first, or turn on `auto_provision` and grant roles after
their first sign-in. See [Roles and access](/docs/guides/roles-and-access/).

The flow uses PKCE, a nonce and single-use server-side state. The discovery
client does not follow redirects.

:::caution
SSO sessions are not treated as MFA-verified. Users who run sensitive actions
also enrol an authenticator app or a passkey and step up when asked. See
[Multi-factor authentication](/docs/guides/mfa/).
:::

## Verify

- The sign-in page shows **Sign in with SSO**.
- **Administration → Profile → Sign-in & MFA policy** shows
  **Single sign-on (OIDC)** as **enabled** with your issuer URL.
- `GET /api/capabilities` lists `auth.oidc` as enabled.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| No **Sign in with SSO** button | Discovery failed at start-up, or `[oidc]` is missing | Check the server log at start-up; check that the server reaches `issuer_url` over HTTPS |
| Provider reports a redirect URI mismatch | The registered URI differs from `<public_url>/api/v1/auth/oidc/callback` | Register the exact URI, including scheme and port |
| Sign-in succeeds at the provider but Forge refuses the user | No linked user, no matching verified email, and `auto_provision = false` | Create the user with the same email, or turn on `auto_provision` |
| The user signs in but sees nothing | A provisioned user has no roles | Grant a role in **Members & roles** |

## Next steps

- [Configuration reference](/docs/reference/configuration/) for every `[oidc]` key.
- [Roles and access](/docs/guides/roles-and-access/)
- [Multi-factor authentication](/docs/guides/mfa/)
