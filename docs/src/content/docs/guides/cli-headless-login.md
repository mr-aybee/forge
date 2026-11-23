---
title: Sign in to the CLI on a server or over SSH
description: Sign in the forge CLI on a machine without a browser by approving a short code in the console from another device.
sidebar:
  order: 4
---

This guide shows you how to sign in the `forge` CLI on a machine that has no
browser, such as a server you reach over SSH or a container. The CLI shows a
short code; you approve it in the Forge console on your laptop or phone.

## Before you begin

- Install the `forge` CLI on the machine. See
  [Command line](/docs/reference/cli/#forge).
- Add a context for your Forge server on that machine:

  ```sh
  forge contexts add <name> --server https://<forge-host>
  ```

  Add `--ca-file <pem>` when the server uses a private CA.
- You need a Forge console account and a second device with a browser that
  reaches the console.
- The sign-in works on Linux, Windows and macOS. Where no credential store is
  available, the CLI keeps the tokens in an owner-only file (see
  [Where the CLI keeps its session](/docs/reference/cli/#where-the-cli-keeps-its-session)).

:::caution
Approve only a code that is on the screen of a device you are using right
now. Someone can send you a link to the approval page with their own code;
if you approve it, they are signed in as you. The approval page shows the
address and client the request came from: check that it is your machine. A
device sign-in is never MFA-verified, whatever your browser session is, so
the device asks for a code before MFA-protected actions.
:::

## Steps

1. On the machine without a browser, start the sign-in:

   ```sh
   forge auth login --device
   ```

   The CLI prints a URL, such as `https://<forge-host>/device`, and a code of
   eight letters, such as `BCDF-GHJK`. It also prints a URL with the code
   filled in. It then waits.

2. On your other device, open the URL. Sign in to the console if it asks you
   to.

3. On the **Sign in a device** page, type the code into **Code** and select
   **Continue**. Case, spaces and the dash do not matter. If you opened the
   URL with the code filled in, select **Continue** as well.

4. Check that the code on the page matches the code in your terminal, and
   that the page names your account.

5. Select **Approve**. Select **Deny** if you did not start this sign-in.

Within a few seconds the CLI prints `Signed in to <name> as <user>` and where
it keeps the token.

## Verify

Run:

```sh
forge auth whoami
```

It prints your user, roles and whether the session is MFA-verified.

## Run MFA-protected actions

Some actions, such as deploying a privileged stack or updating agents, need
an MFA-verified session. If the browser session you approved from was not
MFA-verified, the CLI session is not either. You have two options:

- Step up before you start:

  ```sh
  forge auth step-up
  ```

  Enter the 6-digit code from your authenticator app.
- Run the command. On a terminal, the CLI asks for the code when the action
  needs MFA, steps up and retries once. With `--non-interactive`, it exits
  with code `5` instead.

Step-up in the CLI needs an authenticator app (TOTP). If you have only
passkeys, add an authenticator app to run MFA-protected actions from the CLI,
or run them in the console. See [MFA](/docs/guides/mfa/).

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `the device code expired before it was approved` (exit `8`) | The code lives 10 minutes, and the CLI waits `--timeout-secs` (default `300`) | Run `forge auth login --device` again. Pass `--timeout-secs 600` for more time |
| `the sign-in was denied in the console` (exit `3`) | Someone selected **Deny** | Run the command again and approve it |
| **No pending sign-in with that code** | A typo, or the code expired or was already used | Check the code. If it still fails, run the command again on the machine |
| `too many wrong codes; wait a while` | 10 wrong codes in 15 minutes for your account | Wait 15 minutes, then try again |
| `approve device sign-ins in the Forge console` | The approval came from an app or an API key, not a browser session | Approve in the console in a browser |
| The URL shows the wrong host | `public_url` on the server is not the address you use | Open the console address you normally use and go to `/device`, or ask an administrator to set `public_url` (see [Configuration](/docs/reference/configuration/)) |
| `... is now a different deployment` (exit `7`) | The context's server was reinstalled or points to another Forge | If intended, run `forge contexts remove <name>`, add it again and sign in |
| `interactive sign-in is disabled by --non-interactive` | Device sign-in needs a person | Run it without `--non-interactive`. For scripts, use an API key in `FORGE_TOKEN` |
| `pass the code with --code (no terminal to ask on)` (exit `5`) | `forge auth step-up` without a terminal | Run `forge auth step-up --code <code>` |

The server may ask the CLI to poll less often (`slow_down`). The CLI handles
this by itself; you do not need to do anything.

## Next steps

- [Command line](/docs/reference/cli/#forge-auth): every `forge auth` flag and the
  exit codes.
- [MFA](/docs/guides/mfa/): set up an authenticator app or a passkey.
- [Profile and API keys](/docs/guides/settings-and-api-keys/): API keys for
  automation.
- [REST API](/docs/reference/rest-api/#device-sign-in): the device sign-in
  endpoints.
