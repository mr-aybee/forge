---
title: Profile and API keys
description: Find your way around your Profile, set shared business settings, and create API keys and service-account tokens for scripts and CI.
sidebar:
  order: 7
---

This guide shows you how to find each setting, set shared business settings,
and create and revoke API keys and service-account tokens for scripts and CI.

## Before you begin

- Any signed-in user can open **Administration → Profile** and manage their
  own account, security and API keys. The **Workspace** sections appear only
  to users who may read workspace settings.
- Setting or deleting a business setting needs `workspace.manage` on the
  workspace (**Admin** or **Platform admin**).
- You create API keys in a browser session. An API key cannot create
  another key.
- Service accounts need `access.manage` on the workspace to create one or to
  issue a token, and `user.manage` to revoke a token.

## Where each setting lives

**Profile** has a rail of sections on the left (a row of tabs on a narrow
window). `/profile` opens **General**; old `/settings/<section>` links
redirect here. Server-level settings, such as the public address, are on the
separate **Administration → Settings** page
([Publish Forge on a domain](/docs/guides/publish-forge-on-a-domain/)).

| Group | Section | Panels |
| --- | --- | --- |
| **Personal** | **General** | **Your account**, **Appearance** |
| | **Password & security** | **Password**, **Multi-factor authentication**, **Passkeys** |
| | **Tokens & API keys** | **Personal API keys**, **Service accounts**, **Connected apps** |
| | **Tunnels** | Your open port tunnels with **Close**, and the `forge tunnel open` lines to copy; everyone's tunnels and grants are under **Administration ▸ Tunnels** ([Open a port tunnel](/docs/guides/port-tunnels/)) |
| **Workspace** | **Instance** | Version and the URLs from `forge.toml` (console, node-facing URL, agent endpoint) and whether cookies are `Secure`, plus **About this control plane**. These are bootstrap values: change them in `forge.toml` and restart. |
| | **Sign-in & MFA policy** | Local passwords, single sign-on (see [Single sign-on (OIDC)](/docs/guides/sso-oidc/)), whether this server requires MFA, **MFA adoption** and the actions that need MFA |
| | **Business settings** | Shared key/value pairs such as `company.domain` or `support.email` |
| | **Connectors** | AI connectors registered with the workspace |

## Set a business setting

1. Open **Administration → Profile → Business settings**.
2. Under **Business settings**, enter the **Key** (lowercase letters, digits,
   `_`, `.` and `-`) and the **Value**.
3. Select **Set**.

Services use a setting in environment values as
`${forge.settings.company.domain}`. Forge expands it at deploy time and
refuses unknown names. To delete a setting, open its row menu and select
**Delete**.

Keys that start with `agents.` cannot be set or deleted here (`400`). They
belong to **Administration → Forge updates → Automatic updates**. See
[Update agents automatically](/docs/guides/update-agents/#update-agents-automatically).

## Create an API key

1. Open **Administration → Profile → Tokens & API keys**.
2. Under **Personal API keys**, select **New key**.
3. Enter a **Name**, for example `GitHub Actions deploy`.
4. Pick when it **Expires**: **in 30 days**, **in 90 days** (the default),
   **in a year** or **never**. A key that never expires shows its expiry
   **never** in the warning color; prefer keys that expire.
5. Under **Access**, choose **All my access** or **Only selected**. Prefer
   **Only selected** for automation; see
   [Restrict an API key](#restrict-an-api-key).
6. Leave **Elevated** unticked unless the key must run MFA-guarded actions
   (see below).
7. Select **Create key**.
8. Copy the key now. The full key (`fk_<prefix>_<secret>`) is shown once.
   Forge stores only its SHA-256.

Store the key in your CI's secret store and pass it in an environment
variable:

```sh
curl -H "Authorization: Bearer $FORGE_KEY" https://<forge-host>/api/v1/servers
```

A request with a key needs no CSRF token.

A key with **All my access** acts as its owner with the owner's roles. A normal key is refused
for MFA-guarded actions (terminal sessions, package installs, node
onboarding, Forge updates) with `step_up_required`. An **elevated** key runs
them without a prompt. Ticking **Elevated** asks you to verify with MFA
first, and the key shows an **elevated** badge in the list. When the server
sets `[security] allow_elevated_tokens = false`, the option is disabled and
existing elevated keys act as normal keys.

:::caution
Anyone holding an elevated key can do everything an MFA-verified session of
its owner can, with no second factor. Use one only for automation that needs
it, give it an expiry, keep it in a secret store, and prefer a service
account with only the roles the pipeline needs over a key of an
administrator.
:::

## Restrict an API key

A restricted key may run only the actions you pick, at the scopes you pick
and everything beneath them. It never gets more than you hold: at every
request Forge checks your own access **and** the key's list. If you lose a
role later, the key loses it too. For the model, see
[Restricted tokens](/docs/concepts/authorization/#restricted-tokens).

1. In the **New API key** dialog, under **Access**, select **Only selected**.
2. Select **Add scope** and pick a place in the tree: the workspace, a
   project, an environment, a service, a cluster or a node.
3. Expand the scope and pick its actions. Each area has **None**, **Read**,
   **Write** and **Admin** presets, and you can tick single actions. Only the
   actions you hold there are offered.
4. Repeat for more scopes, up to 50. Every scope needs at least one action.
5. Select **Create key**.

With the CLI, repeat `--allow` once per scope:

```sh
forge api-keys create ci-uat --expires-days 30   --allow environment:<environment-id>=view,deploy,job.create
```

:::note
A job started with a restricted key is checked against the key's list when
it is submitted. Its later steps re-check your own access, not the key's
list, for the same action and resource. Service-account tokens cannot be
restricted this way; give the service account a narrow assignment instead.
:::

## Issue a service-account token

1. Open **Administration → Profile → Tokens & API keys**.
2. Under **Service accounts**, select **New service account**, enter a
   **Name** (the account is called `svc-<name>`) and a **Description**, and
   select **Create**. You become its accountable owner.
3. Select the account's name (or **Tokens** in its row menu).
4. Enter a **Token name**, pick when it **Expires**, tick **Elevated** only if
   needed, and select **Issue token**.
5. Copy the token now. It is shown once.

To revoke a token, open it in the same dialog, select **Revoke token** in its
row menu and confirm with **Revoke**.

## Revoke an API key

Under **Personal API keys**, open the key's row menu, select **Revoke key** and
confirm with **Revoke**. Anything using the key stops working immediately.

A platform admin can list and revoke another user's keys, for example after
a leak: `forge api-keys list --user <user-id>`, `forge api-keys delete
<key-id> --user <user-id>` or `forge api-keys revoke-all --user <user-id>`.
Revoking needs no MFA. `forge-server reset-mfa` and removing all of a user's
passkeys revoke their keys too; an ordinary password change keeps them, so
CI jobs keep working.

## Verify

- The new key is listed under **Personal API keys** with its name and
  expiry, and an **elevated** badge if you ticked **Elevated**.
- For a restricted key, `forge api-keys list` shows its entries in the
  `RESTRICTED` column, and a request outside them returns `403` with
  `outside what this token was given`.
- `curl -H "Authorization: Bearer $FORGE_KEY" https://<forge-host>/api/v1/me`
  returns your user.
- **Operations → Audit log** shows `apikey.create` (and `apikey.revoke` after
  revoking).

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `403` "API keys cannot create API keys" | The request used an API key | Create keys in the console |
| `403` `step_up_required` for an MFA-guarded action | MFA is required on this server, and the key is not elevated | Create an elevated key, or run the action in the console or with the `forge` CLI signed in to a context |
| **Elevated** is disabled | The server sets `[security] allow_elevated_tokens = false` | Use a normal key, or ask the operator to change `forge.toml` |
| No **Service accounts** panel | You lack permission to see service accounts, or the server is older | Ask an administrator |
| `403` `escalation` when creating a restricted key | The key lists an action you do not hold over that scope | Remove the action, or ask for it first |
| `403` "…: outside what this token was given" | A restricted key tried an action, or a resource, its list does not cover | Create a key that lists it |
| A restricted key sees fewer items in a list | Lists show only what both you and the key may see | Add the scope to a new key |
| `401` with a key that worked before | The key expired or was revoked | Create a new key |
| `400` when setting a key that starts with `agents.` | Those keys have their own endpoints | Use **Administration → Forge updates** |

## Next steps

- [API reference](/docs/reference/rest-api/): `/settings`, `/settings/{key}`,
  `/api-keys`, `/api-keys/{id}` and `/users/{id}/api-keys`.
- [`forge api-keys`](/docs/reference/cli/api-keys/)
- [Roles and access](/docs/guides/roles-and-access/)
- [Access profiles, groups and service accounts](/docs/guides/access-profiles/)
- [Sign in to the CLI on a server or over SSH](/docs/guides/cli-headless-login/)
