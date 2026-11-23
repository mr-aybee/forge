---
title: Sign in from a desktop or mobile app
description: Sign a native app in to Forge with the OAuth authorization code flow and PKCE, and adapt to each control plane's version.
sidebar:
  order: 5
---

This guide shows you how a native app signs a user in to a Forge control
plane with OAuth 2.1 (authorization code + PKCE) and calls the API with the
resulting token. The `forge` CLI uses the same flow.

:::note
Forge does not ship a desktop or mobile app. This guide is for developers who
build one against the REST API.
:::

## Before you begin

- The user has a Forge account. The app acts with that user's roles.
- The app uses the built-in public client `forge-app`. Its allowed redirect
  URIs are `http://127.0.0.1:<any-port>/callback` (loopback) and
  `orbynforge://auth/callback`.
- The app can open the system browser and either listen on a loopback port
  or handle the `orbynforge://` scheme.

## Steps

1. Discover what the control plane supports. Both endpoints are public:

   ```http
   GET /api/version
   GET /api/capabilities
   → {"capabilities": [{"id": "auth.oidc", "version": 1, "enabled": true}, ...]}
   ```

   Turn features on per capability, so one app can manage control planes of
   different versions.
2. Generate a `code_verifier` (43–128 characters) and
   `code_challenge = BASE64URL(SHA256(verifier))`.
3. Open the system browser at:

   ```text
   <forge-url>/oauth/authorize?response_type=code&client_id=forge-app
     &redirect_uri=http://127.0.0.1:<port>/callback
     &code_challenge=<challenge>&code_challenge_method=S256&state=<random>
   ```

   The user signs in to the console, with MFA if enrolled.
4. Receive `code` on the redirect URI and check that `state` matches.
5. Exchange the code for tokens:

   ```http
   POST /oauth/token  (application/x-www-form-urlencoded)
   grant_type=authorization_code&code=<code>&code_verifier=<verifier>&client_id=forge-app&redirect_uri=<redirect-uri>
   → {"access_token": "…", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "…"}
   ```

6. Call the API with `Authorization: Bearer <access_token>`. Bearer requests
   need no CSRF token.
7. Before the access token expires (one hour), refresh it with
   `grant_type=refresh_token`. Refresh tokens rotate: each one works once.
   A refresh token stops working `[security] refresh_token_days` after the
   original sign-in (30 days by default); the user then signs in again.

:::caution
Store tokens in the operating system's keychain, never in plain files or
logs. The app's tokens carry the MFA state of the browser session that
authorised them.
:::

## Verify

- `GET /api/v1/me` with the access token returns the user.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `bad redirect_uri` or the browser shows an error before sign-in | The redirect URI is not one the client allows | Use `http://127.0.0.1:<port>/callback` or `orbynforge://auth/callback` |
| `invalid_grant` on the token exchange | The code was used twice, expired, or the verifier does not match | Start again with a new verifier and code |
| `401` after about an hour | The access token expired | Refresh it |
| Refresh fails with `invalid_grant` | The refresh token was already used, or reached its absolute lifetime | Sign the user in again |
| An MFA-guarded call is refused | The authorising browser session was not MFA-verified | Have the user step up in the console and sign in again |

## Next steps

- [REST API](/docs/reference/rest-api/) for every endpoint.
- [Sign in to the CLI on a server or over SSH](/docs/guides/cli-headless-login/)
  for the device flow, for machines without a browser.
- [Connect AI agents](/docs/guides/connect-ai-agents/) for MCP clients that register
  their own OAuth client.
