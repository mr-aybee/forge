---
title: Access profiles, groups and service accounts
description: Give people, groups and automation exactly the access they need with access profiles, at any level from the workspace down to one service.
sidebar:
  order: 31
---

Use an access profile when none of the built-in roles fits: for example
"view the UAT cluster and manage its servers, but no terminal". Use a group
to give the same access to several people at once. Use a service
account for a CI pipeline or a script, so it runs as its own identity
instead of as a person. For how profiles, groups, assignments and service
accounts work, see [Authorization](/docs/concepts/authorization/#access-profiles).

## Before you begin

- Creating and changing profiles, and managing service accounts, needs
  `access.manage`: **Platform admin**, or **Admin** in the workspace.
- Giving a profile to someone needs `user.manage` on the scope (**Admin**
  inside the workspace, or **Platform admin**), and you must hold every
  action of the profile there yourself.
- Creating, renaming and deleting groups needs `access.manage` on the
  workspace. Adding and removing members needs `user.manage` on the
  workspace. Adding a member also needs every action the group holds, at
  every scope where the group is assigned.
- All of these need an MFA-verified session. Set up an authenticator or a
  passkey first; see [Multi-factor authentication](/docs/guides/mfa/). Removing
  access (an assignment or a group member) never needs MFA.
- The examples use the [`forge` CLI](/docs/reference/cli/), signed in as an
  administrator. The same actions are in the
  [REST API](/docs/reference/rest-api/), and in the console on
  **Administration → Access Control → Access** (`/access`).

In the console, a profile is called a **role**.

## Work on the Access page

**Access** is one page for users, groups and roles. Changes you make there
are staged first and written only when you apply them.

1. Open **Administration → Access Control → Access**.
2. Pick **Users**, **Groups** or **Roles** in the toggle, then pick an item in
   the list on the left.
   - For a user or a group, the right side lists every resource with the
     **Roles here**. Roles held through a parent scope or a group are shown
     muted. Expand a row to see the actions allowed there.
   - For a role, the right side shows its permissions and where it is
     **Assigned**.
3. Make changes: **+ Add** on a resource row stages a role, **×** on a tag
   stages its removal, and the role pane stages permission edits. Staged
   tags are marked; select **×** on a staged tag to drop it.
4. Select **Review (n)** to read each change as a sentence. Drop any you do
   not want.
5. Select **Apply changes** (or **Apply n changes** in the review). Forge
   runs them in order and shows a result per change. A refused change stays
   staged with the reason; fix it or drop it.

The **Manage** menu holds the rest: **All roles**, **New role**,
**All groups**, **New group**, **Who has access…** (pick a resource to see
every user and group holding a role there), **Members** (invite and
disable people) and **Tokens** (your personal API keys).

## Create a profile

1. List the actions a profile can hold and which of them need MFA:

   ```sh
   forge access catalog
   ```

2. Create the profile with exactly the actions it should allow:

   ```sh
   forge access profiles create "UAT support" --action view --action server.manage \
     --description "UAT on-call: see and manage servers, no terminal"
   ```

   To start from a built-in role, copy it instead and change the copy:

   ```sh
   forge access profiles duplicate Operator "Night operator"
   ```

In the console, on **Access**, select **Manage → New role**, enter a
**Name** and **Description**, pick the permissions and create it. To start
from a built-in role, pick it under **Roles** and select **Duplicate**.
Permissions are grouped by area. Each area has **None**, **Read**,
**Write** and **Admin** presets, and you can tick single actions. A preset
selects only actions that exist now; it never adds actions Forge introduces
later.

## Give it to a person

1. Assign the profile at the scope it should cover. The assignment covers
   the scope and everything under it.

   ```sh
   forge access assignments add alice "UAT support" cluster:uat --yes
   ```

2. For temporary access, add an expiry in minutes. Forge removes the
   assignment within 30 seconds of it.

   ```sh
   forge access assignments add bob "UAT support" cluster:uat --expires-in-minutes 480 --yes
   ```

In the console, on **Access**, select **Users** and the person, find the
resource row, select **+ Add**, pick the role (and an **Expiry** for
temporary access), then **Review** and **Apply changes**.

### Choose the level

Pick the narrowest scope that does the job.

| Console level | Scope argument | Covers |
| --- | --- | --- |
| Workspace | `workspace:<workspace-id>` | Everything in the workspace |
| Project | `project:<project-id>` | Every environment of the project and their services |
| Environment | `environment:<environment-id>` | The environment, its services and their jobs |
| Service | `service:<service-id>` | One service and its jobs |
| Cluster | `cluster:<cluster-id or name>` | The cluster and its nodes, not the environments placed in it |
| Node | `server:<node-id>` | One node |

Node-only actions such as `container.manage` act on nodes. You can still
assign a profile that holds them at an environment or a cluster: they take
effect on the nodes under that scope. See
[Held over a scope](/docs/concepts/authorization/#held-over-a-scope).

### Example: let someone restart one service

To let `alice` see, restart and redeploy one service and nothing else:

1. Create a profile with the three actions a restart needs:

   ```sh
   forge access profiles create "Service restarter" --action view --action deploy --action job.create
   ```

2. Assign it at the service:

   ```sh
   forge access assignments add alice "Service restarter" service:<service-id> --yes
   ```

`alice` now sees the service in the services list, opens it, reads its
status and logs, and can start, stop, restart and deploy it. She sees no
other service in the environment.

:::note
A container service that mounts host paths outside its own folder
(privileged mounts) still needs `stack.privileged` on the environment to
deploy. An assignment at the service is not enough for that.
:::

## Give access to a group

A group gets profiles the way a person does, and every member holds them. A
user can be in many groups; their access is the union of everything they
hold directly and through each group.

1. Create the group:

   ```sh
   forge access groups create uat-oncall --description "UAT on-call rota"
   ```

2. Assign profiles to it with `--group`. The first argument is then the
   group's name or id:

   ```sh
   forge access assignments add uat-oncall "UAT support" cluster:uat --group --yes
   ```

3. Add members:

   ```sh
   forge access groups add-member uat-oncall alice
   forge access groups add-member uat-oncall bob@example.com
   ```

In the console, on **Access**, select **Manage → New group**. Then, with the
group selected under **Groups**, add roles with **+ Add** on resource rows
and members with **+ Add member** under **Members**, and apply the changes.
From a user, **+ Add to group** under **Groups** does the same.

:::caution
Adding someone to a group gives them every profile the group holds, at once,
wherever it is assigned. Check the group's assignments before you add a
member.
:::

## Run automation as a service account

1. Create the service account. It is named `svc-<name>`, never signs in and
   can do nothing yet.

   ```sh
   forge service-accounts create ci-deploy --description "GitHub Actions deploys"
   ```

2. Give it a profile at the narrowest scope that works, for example the
   environment it deploys to:

   ```sh
   forge access assignments add svc-ci-deploy Developer environment:<environment-id> --yes
   ```

3. Issue a token and store it in your CI's secret store. Forge shows the
   secret once and keeps only a hash. Tokens expire after 90 days unless
   you choose otherwise.

   ```sh
   (umask 077; forge -o json service-accounts token-create ci-deploy gha | jq -r '.result.secret' > gha.token)
   ```

4. In the pipeline, pass the token to the CLI or the API:

   ```sh
   export FORGE_SERVER=https://<forge-host>
   export FORGE_TOKEN="$(cat gha.token)"
   forge services deploy <service>
   ```

:::caution
A token acts as the service account until it expires or you revoke it.
Keep each account's profile as narrow as the job allows, issue one token per
pipeline, and revoke tokens you no longer use. Tokens never hold MFA, so
MFA-guarded actions (terminals, package installs, vault reveals) stay out of
reach of automation unless MFA is turned off for the server.
:::

## Change a profile

1. Preview what a change would do: the actions it adds and removes, how
   many assignments and people it affects (group members included), and any
   scopes where you lack an added action.

   ```sh
   forge access profiles publish "UAT support" --action view --dry-run
   ```

2. Publish it. Every assignment moves to the new revision at once.

   ```sh
   forge access profiles publish "UAT support" --action view --yes
   ```

In the console, pick the role under **Roles** and change its permissions.
The edit is staged as a new revision; **Review** shows how many people it
affects before you apply it. Built-in roles are read-only.
## Remove access

In the console, select **×** on the role tag or member tag, then **Review**
and **Apply changes**. With the CLI:

- One assignment: `forge access assignments remove <assignment-id> --yes`.
- One group member: `forge access groups remove-member uat-oncall alice`.
  Their access through the group ends at once.
- A whole group: `forge access groups delete uat-oncall --yes`. Its
  assignments are removed and every member loses access through it at once.
- A whole profile: `forge access profiles archive "UAT support" --yes`. Its
  assignments stop granting at once.
- A service account: `forge service-accounts disable ci-deploy --yes`
  revokes all its tokens at once (enabling it later needs new tokens); `forge service-accounts token-revoke ci-deploy <token-id> --yes`
  ends one token.

## Verify

- `forge access assignments list` shows the assignment with its user or
  group, scope and expiry.
- `forge access effective alice service:<service-id> --allowed` lists
  every action `alice` holds there, and the assignments behind them, direct
  or through a group. In the console, on **Access**, pick the user and
  expand the resource row. **Manage → Who has access…** lists everyone with
  a role on a resource.
- The person, or the pipeline with its token, can do what the profile
  allows at that scope and is refused (`403`) for anything else.
- **Operations → Audit log** shows `access.profile.create`,
  `access.assign`, `access.group.create`, `access.group.member.add`,
  `service_account.create` and `service_account.token.create`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `403` `step_up_required` when creating, publishing, assigning or adding a member | These need an MFA-verified session | Step up with your authenticator code or passkey, then try again |
| `403` `escalation` when assigning | The profile holds an action you don't hold at that scope | Ask someone who holds it there, or assign a narrower profile |
| `403` `escalation` when adding a group member | The group holds actions you don't hold at one of its scopes; the message names the scope and the actions | Ask someone who holds them there to add the member |
| `400` "give a group_id or a user, not both" | The request named a group and a user | Send one of `group_id`, `user_id` or `login` |
| `409` "a group named … exists" | Group names are unique, ignoring case | Choose another name |
| `409` `escalation` when publishing | The new revision adds an action you lack at a scope where the profile is assigned | Run it with `--dry-run` to see the `blocked` scopes |
| `409` "built-in profiles are read-only" | Built-in profiles mirror the roles | Duplicate it and change the copy |
| `409` "the profile is at revision N now" | Someone published meanwhile | Look at it with `forge access profiles show`, then publish again |
| A person with a profile at one service can't restart it | A restart runs as a job: it needs `view`, `deploy` and `job.create` at the service | Add the missing actions; check with `forge access effective` |
| A change shows **refused** after **Apply changes** | The server refused it, for example `escalation` or `step_up_required`; the reason is under the change | Fix the cause and apply again, or drop the change |
| A pipeline gets `401` | Its token expired or was revoked, or the account is disabled | Issue a new token, or enable the account |
| A pipeline gets `403` for an MFA-guarded action | Tokens never hold MFA | Do that step in the console, or as a person with the CLI |

## Next steps

- [Authorization](/docs/concepts/authorization/#access-profiles): the model.
- [`forge access`](/docs/reference/cli/access/) and
  [`forge service-accounts`](/docs/reference/cli/service-accounts/): every command
  and flag.
- [Roles and access](/docs/guides/roles-and-access/): the built-in roles.
- [Restrict an API key](/docs/guides/settings-and-api-keys/#restrict-an-api-key):
  limit a personal token to some actions and scopes.
