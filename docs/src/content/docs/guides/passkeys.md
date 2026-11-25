---
title: Passkeys
description: Add a passkey to your account, sign in and step up with it, and recover when a passkey is lost.
sidebar:
  order: 3
---

A passkey lets you sign in to the console without a password and confirm
sensitive actions without typing a code. Forge treats every passkey as
multi-factor authentication, because the authenticator always checks your
PIN or biometrics.

For how Forge verifies passkeys, see
[Security model](/docs/concepts/security-model/#how-forge-verifies-passkeys). For
every endpoint and field, see [REST API](/docs/reference/rest-api/#passkeys).

## Before you begin

- The console must be served over **HTTPS**, or opened as `localhost`.
  Browsers offer passkeys only on secure origins.
- `public_url` in the [configuration](/docs/reference/configuration/) must be the
  exact URL you open the console at. Forge uses its host as the passkey
  domain (the WebAuthn relying party ID) and its origin to check every
  response.
- You need a browser or authenticator that supports passkeys with user
  verification: for example Windows Hello, macOS or iOS Touch ID and Face ID,
  Android screen lock, a password manager that stores passkeys, or a FIDO2
  security key with a PIN.
- You need a signed-in browser session. Passkeys are added in the console,
  not with an API key.
- No role or Cedar action is needed to manage your own passkeys.

:::caution
A passkey belongs to the host name you register it on: the address in your
browser. Forge answers on any name, but a passkey made on `forge.example.com`
is not offered on `192.0.2.10` or `ops.example.com`. There, sign in with your
password and code, or add a passkey on that name too. Browsers offer
passkeys only on HTTPS or `localhost`.
:::

## Add a passkey

1. Open **Administration → Profile → Password & security**. Under **Passkeys**, select
   **Add passkey**.
2. In **Add a passkey**, enter a **Name** that tells you which device holds the passkey, for example
   `Work laptop`. Leave it empty to use `Passkey`. Names have up to 60
   characters.
3. Follow your browser's prompt: choose where to save the passkey, then
   unlock it with your PIN, fingerprint or face.

Forge stores the passkey's public key, its name and the dates it was added
and last used. You can register up to 20 passkeys per account.

## Sign in with a passkey

1. Open the console sign-in page and select **Sign in with a passkey**.
2. Pick your account in the browser's prompt and unlock the passkey.

You don't enter a username or password. The session is MFA-verified, as if
you had signed in with a password and an authenticator code.

## Step up with a passkey

Some actions, such as opening a terminal or installing packages, need an
MFA-verified session. If yours is not, Forge shows **Verify it's you** when
you try one.

1. In **Verify it's you**, select **Use a passkey**.
2. Unlock one of your passkeys when the browser asks.

Forge replaces your session with an MFA-verified one and retries the action.
You can use a passkey or an authenticator code here, whichever you have. See
[Multi-factor authentication](/docs/guides/mfa/).

## Rename or remove a passkey

1. Open **Administration → Profile → Password & security** and find the passkey under
   **Passkeys**.
2. To rename it, open the row's actions menu (or right-click the row), select **Rename**, enter the
   new **Name** and save. Names have 1 to 60 characters.
3. To remove it, open the row's actions menu, select **Remove** and
   confirm.

Removing a passkey on Forge does not delete it from your device. Delete it
there too, in your browser's or operating system's passkey settings.

## Verify

- **Profile → Password & security → Passkeys** lists the passkey with its name, the date
  it was added and, after the first use, the date it was last used.
- Sign out, then sign in with **Sign in with a passkey**. You reach the
  console without a password.
- `GET /api/v1/me` returns `"mfa": true` after a passkey sign-in or step-up.
- The [audit log](/docs/concepts/audit/) shows `auth.passkey.add` when you added
  it, and `auth.login` with `"method": "passkey"` when you signed in with it.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| The console says **Passkeys need HTTPS (or localhost).**, or the browser offers no passkey prompt | The page is not on a secure origin. Browsers offer WebAuthn only on HTTPS or `localhost`. | Serve the console over HTTPS behind a TLS-terminating proxy, and set `public_url` to the `https://` URL. |
| The browser reports a security error, or Forge answers `the passkey response does not match this sign-in: origin` or `relying party` | You opened the console at a URL other than `public_url` (another host name, port or scheme). | Open the console at `public_url` exactly, or correct `public_url` and restart Forge. |
| `the passkey response does not match this sign-in: user verification (PIN or biometrics) is required` | The authenticator did not verify you. Some security keys have no PIN set, and some browsers skip verification. | Set a PIN on the security key, or use an authenticator with biometrics or a device PIN. Forge always requires user verification. |
| `this passkey sign-in expired or was already used; try again` | Each passkey prompt is valid once, for 5 minutes. The prompt was left open too long, or the same response was sent twice. | Start again. Forge issues a new challenge each time. |
| `the passkey's counter went backwards; it may have been cloned` | The authenticator reported a signature counter that is not higher than the last one Forge saw. This happens when a hardware key's secret was copied, or after the authenticator was reset. | Treat it as a possible compromise. Remove the passkey under **Profile → Password & security → Passkeys**, register a new one, and review the [audit log](/docs/concepts/audit/) for sign-ins you don't recognise. |
| `unknown passkey` | The passkey was removed from Forge, or it belongs to another Forge installation. | Sign in another way and add the passkey again. |
| `at most 20 passkeys per account` | You reached the limit. | Remove passkeys you no longer use. |
| `429 throttled` | 10 failed attempts within 15 minutes for this passkey (sign-in) or your account (step-up). | Wait 15 minutes. |
| You lost the device that holds your passkey | The passkey is gone with the device. | Sign in with another passkey, or with your password and authenticator code, and remove the lost passkey. If you have no other way in, follow [Recover access](#recover-access). |

## Recover access

When a user has lost every passkey, an administrator removes them. The user
then signs in with their password and adds new ones.

### An administrator removes someone's passkeys

You need `user.manage` on the platform (role
**Platform admin**) and an MFA-verified session.

1. Open **Administration → Members & roles**, open the user's row menu and
   select **Remove passkeys**.
2. Confirm with **Remove passkeys**.

The same through the API:

```sh
curl -X DELETE "https://<forge-host>/api/v1/users/<user-id>/passkeys" \
  -H "Authorization: Bearer $FORGE_TOKEN"
```

The answer is `{"removed": <count>}`. Forge records
`auth.passkey.remove_all`. This removes passkeys only; the user's
authenticator app stays enrolled.

### A platform admin lost every factor

If no administrator can sign in with MFA, reset the account on the server
host, with the configuration file the service uses.

On Linux, as the `forge` user that owns the data directory:

```sh
sudo -u forge /opt/forge/forge-server --config /etc/forge/forge.toml reset-mfa <username-or-email>
```

On Windows, in PowerShell as Administrator:

```powershell
& "C:\Program Files\Orbyn Forge\forge-server.exe" --config "C:\ProgramData\Orbyn Forge\forge.toml" reset-mfa <username-or-email>
```

The command:

- removes all of the user's passkeys,
- removes their authenticator app (TOTP),
- revokes their API keys (a key made from a stolen session would outlive it),
- signs out every session of that user,
- records `auth.mfa.reset` in the audit log with the actor `system:forge`.

The user then signs in with their password and enrolls MFA again under
**Administration → Profile → Password & security**.

:::caution
`reset-mfa` needs shell access to the server host and read access to the
database and keys. Anyone with that access can already control Forge, so the
command adds no new path in. Restrict who can log in to the server host, and
review `auth.mfa.reset` entries in the audit log.
:::

## Next steps

- [Multi-factor authentication](/docs/guides/mfa/): authenticator apps and which
  actions need MFA.
- [Security model](/docs/concepts/security-model/#how-forge-verifies-passkeys):
  what Forge checks on every passkey response.
- [REST API](/docs/reference/rest-api/#passkeys): passkey endpoints for custom
  clients.
- [Command line](/docs/reference/cli/#forge-server): `forge-server reset-mfa`.
