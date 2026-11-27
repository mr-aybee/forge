---
title: Multi-factor authentication
description: Enroll an authenticator app or a passkey and use step-up MFA for sensitive actions.
sidebar:
  order: 3
---

This guide shows you how to protect your account with an authenticator app
(TOTP) and how to verify yourself when an action needs MFA. Forge has two
second factors: an authenticator app and [passkeys](/docs/guides/passkeys/).
Either one gives you an MFA-verified session.

## Before you begin

- Any signed-in user can enroll. No role is needed.
- You need an authenticator app (any TOTP app) or a device that supports
  passkeys.
- MFA is your own choice. Once you enroll a factor, Forge asks for it every
  time you sign in.
- Whether sensitive actions also need MFA is the server operator's choice:
  `[security] require_mfa` in `forge.toml`, `false` by default. While it is
  off, no action asks for MFA and the step-up below does not happen. See
  [MFA is your choice](/docs/security/#mfa-is-your-choice).

## Enroll an authenticator app

1. Open **Administration → Profile → Password & security**.
2. Under **Multi-factor authentication**, select **Set up authenticator app**.
3. Scan the QR code with your app, or type the key shown under it.
4. Enter the code from the app in **Code from the app**. If this is your
   first sign-in factor (no authenticator app and no passkey yet), also
   enter your **Current password**. Accounts that sign in through single
   sign-on have no password: set the app up within 5 minutes of signing in.
   Select **Verify and enable**.

   :::caution
   For an account without a password, a session cookie stolen within 5
   minutes of a single sign-on sign-in is enough to add the first factor.
   This is inherent to accounts that sign in only through SSO.
   :::
5. Sign out and sign in again with a code. The session you enrolled in is
   not MFA-verified until you do, or until you step up (below).

From then on, every password sign-in asks for a **One-time code**. Forge
stores the secret envelope-encrypted.

To use a passkey instead, or as well, see [Passkeys](/docs/guides/passkeys/).

## Verify yourself for a sensitive action (step-up)

This applies only when the server requires MFA (`[security] require_mfa = true`).

If your session is not MFA-verified and you start a sensitive action, Forge
shows **Verify it's you**. Confirm with a passkey or a 6-digit code,
whichever you have. Forge upgrades the session and retries the action. You do
not need to sign out.

Revoking a node always asks for a fresh authenticator code, even in a
verified session.

The `forge` CLI asks for the code itself when a command needs MFA, or you run
`forge auth step-up`. See
[Sign in to the CLI on a server or over SSH](/docs/guides/cli-headless-login/).

## Which actions need MFA

The guardrail policy (`policies/guardrails.cedar`) requires an MFA-verified
session for these Cedar actions:

| Area | Actions |
| --- | --- |
| Nodes | `server.command`, `process.manage`, `node.onboard`, `terminal.open` |
| Software | `package.manage`, `stack.privileged`, `plugin.manage`, `system.update`, `analytics.configure` |
| Files | `file.write`, `file.delete`, `file.admin` |
| Network | `proxy.manage`, `network.connect`, `network.manage`, `certificate.use` |
| Secrets and data | `secret.read`, `db.admin` |

An API key has no second factor behind it, so where MFA is required a
normal key cannot run these actions. A key or service-account token created
as **elevated** can, while `[security] allow_elevated_tokens` is on: create
it from an MFA-verified session (`forge api-keys create --elevated`). The
audit log shows such calls with `token_id` and `elevated: true`. Where the
server does not require MFA (the default), every key counts as MFA-verified like a browser
session (see
[MFA is your choice](/docs/security/#mfa-is-your-choice)). App tokens
obtained through OAuth inherit the MFA state of the browser session that
authorised them. See the [Cedar schema](/docs/reference/cedar-schema/) for which
roles hold each action.

## Verify

- **Profile → Password & security** shows **Authenticator app enrolled.** under
  **Multi-factor authentication**.
- After you sign in with a code, the **Your account** panel under **Profile → General** shows the
  **MFA verified** badge instead of **password only**.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| "This session was not MFA-verified" under **Multi-factor authentication** | You enrolled during this session | Sign out and sign in with a code, or step up when asked |
| "enter your current password to set up an authenticator" | This is your first factor, and Forge asks for the password so a stolen session can't add one | Enter your **Current password** |
| "verify it's you with your code or passkey (step up) first" | You already have an authenticator app or a passkey, and this session did not verify it. This applies even when MFA is turned off for the server | Step up with your current code or passkey, then set up the new app |
| "sign out and sign in again with single sign-on, then add it within 5 minutes" | Your account signs in through single sign-on and has no password to confirm | Sign out, sign in again with SSO, and set up the app within 5 minutes |
| Code is refused | The device clock is off, or the code was already used | Sync the phone's clock; wait for the next code. Each code works once |
| Step-up is refused even with a correct code | 10 failed attempts within 15 minutes lock step-up for the account | Wait for the window to pass |
| Lost the device with the authenticator app | No second factor left | Sign in with a passkey, or ask the server operator to run `forge-server reset-mfa <login>`. See [Passkeys](/docs/guides/passkeys/#recover-access) |
| Sensitive actions fail with an API key | MFA is required on this server, and a normal key carries no second factor | Use an elevated key, or the console or the `forge` CLI signed in with a context |
| **Set up an authenticator app first** when you step up | Your account has no second factor yet | Open **Profile → Password & security** (`/profile/security`) and add an authenticator app or passkey |

## Next steps

- [Passkeys](/docs/guides/passkeys/)
- [Roles and access](/docs/guides/roles-and-access/)
- Sign-in endpoints (`/api/v1/auth/login`, `/auth/totp`, `/auth/step-up`):
  [REST API](/docs/reference/rest-api/)
