---
title: forge access
description: "Role bindings, access profiles, groups and assignments: who may do what, where."
sidebar:
  order: 13
---

`forge access` manages role bindings (a user, a built-in role and a scope),
access profiles (named, versioned sets of actions) with their assignments,
user groups, and the effective access of a user at a scope. For the model behind roles and scopes, see
[Authorization](/docs/concepts/authorization/#roles); for the task, see
[Roles and access](/docs/guides/roles-and-access/). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge access list`](#forge-access-list) | Lists role bindings: yours, plus those on scopes where you hold `user.manage` |
| [`forge access grant`](#forge-access-grant) | Gives a user a role on a scope |
| [`forge access revoke`](#forge-access-revoke) | Removes a role binding by id |
| [`forge access catalog`](#forge-access-catalog) | Lists every action, whether it needs MFA and which roles hold it |
| [`forge access profiles list`](#forge-access-profiles-list) | Lists access profiles |
| [`forge access profiles show`](#forge-access-profiles-show) | Shows a profile and its revisions |
| [`forge access profiles create`](#forge-access-profiles-create) | Creates a profile |
| [`forge access profiles publish`](#forge-access-profiles-publish) | Publishes a new revision, or previews it |
| [`forge access profiles duplicate`](#forge-access-profiles-duplicate) | Copies a profile |
| [`forge access profiles archive`](#forge-access-profiles-archive) | Archives a profile |
| [`forge access assignments list`](#forge-access-assignments-list) | Lists assignments |
| [`forge access assignments add`](#forge-access-assignments-add) | Gives a user, service account or group a profile at a scope |
| [`forge access assignments remove`](#forge-access-assignments-remove) | Removes an assignment |
| [`forge access groups list`](#forge-access-groups-list) | Lists groups with their members |
| [`forge access groups create`](#forge-access-groups-create) | Creates an empty group |
| [`forge access groups delete`](#forge-access-groups-delete) | Deletes a group and its assignments |
| [`forge access groups add-member`](#forge-access-groups-add-member) | Adds a user to a group |
| [`forge access groups remove-member`](#forge-access-groups-remove-member) | Removes a user from a group |
| [`forge access effective`](#forge-access-effective) | Shows every action a user holds at a scope, and why |

Renaming a group has no command; use the console or
`forge api call access.update_group -p id=<group-id> --body '{"name": "<new-name>"}' --yes`.

### forge access list

Lists role bindings. You see your own bindings, plus every binding on a scope
where you hold `user.manage`. Platform administrators see all of them.

**Synopsis**

```text
forge access list
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every binding you can see
forge access list
# Only the platform_admin bindings, for a script
forge -o json access list | jq '.result[] | select(.role == "platform_admin")'
```

**Output**

```text
$ forge access list
ID                                    USER                                  ROLE            KIND      SCOPE
01900000-0000-7000-8000-000000000002  01900000-0000-7000-8000-000000000003  platform_admin  platform  root
```

```json
$ forge -o json access list
{
  "result": [
    {
      "created_at": 1794554225245,
      "id": "01900000-0000-7000-8000-000000000002",
      "role": "platform_admin",
      "scope": {
        "id": "root",
        "kind": "platform"
      },
      "user_id": "01900000-0000-7000-8000-000000000003"
    }
  ],
  "schema": "forge.cli/v1"
}
```

`USER` is the user's id; match it with
[`forge users list`](/docs/reference/cli/users/#forge-users-list).

Exit codes:

- `0`: success.
- `3`: not signed in.

**See also**: [`forge access grant`](#forge-access-grant),
[`forge access revoke`](#forge-access-revoke).

### forge access grant

Gives a user a role on a scope. Needs `user.manage` on that scope. The
command asks for confirmation first.

**Synopsis**

```text
forge access grant [OPTIONS] <USER> <ROLE> <SCOPE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<user>` | username, email or id | — | yes | The user who gets the role. A name must match exactly one account | `alice` |
| `<role>` | enum: `platform_admin`, `workspace_admin`, `operator`, `developer`, `dba`, `auditor`, `customer` | — | yes | The role. See [Roles](/docs/concepts/authorization/#roles) for what each one can do | `operator` |
| `<scope>` | string | — | yes | Where the role applies: `platform:root`, `workspace:<id>`, `tenant:<id>`, `cluster:<id or name>` or `environment:<id>`. Only a cluster takes a name; the CLI looks up its id | `cluster:prod-eu` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

A role on a scope covers everything inside it: a binding on a cluster covers
its environments. `platform_admin` can only be bound at `platform:root`.

**Examples**

```sh
# Make alice an operator on one cluster, by name
forge access grant alice operator cluster:<cluster-name>
# Give a developer access to one environment, without a prompt
forge access grant bob@example.com developer environment:<environment-id> --yes
# Make a second platform administrator
forge access grant carol platform_admin platform:root
```

**Output**

The command prints the new binding. Shape from the server's schema; not
captured on a live system.

```json
$ forge -o json access grant alice operator cluster:<cluster-name> --yes
{
  "result": {
    "created_at": 1794554225245,
    "id": "<binding-id>",
    "role": "operator",
    "scope": {
      "id": "<cluster-id>",
      "kind": "cluster"
    },
    "user_id": "<user-id>"
  },
  "schema": "forge.cli/v1"
}
```

Without `-o json`, the same object prints as a key/value list.

Exit codes:

- `0`: the binding was created.
- `1`: you answered anything but `yes` at the prompt.
- `2`: an unknown role, a malformed scope or a scope kind other than the five
  above, `platform_admin` on a scope other than `platform:root`, a name that
  matches more than one user or cluster, or `--yes` missing without a
  terminal.
- `3`: not signed in.
- `4`: you lack `user.manage` on the scope.
- `6`: the user or the scope does not exist.

:::caution
A binding takes effect at once, on every node and API call in its scope.
Grant the narrowest scope and role that does the job.
:::

**See also**: [`forge access revoke`](#forge-access-revoke),
[`forge users create`](/docs/reference/cli/users/#forge-users-create),
[Roles and access](/docs/guides/roles-and-access/).

### forge access revoke

Removes a role binding by its id. Needs `user.manage` on the binding's scope.
The command asks for confirmation first.

Forge refuses to remove the last `platform_admin` binding, and refuses to let
a platform administrator remove their own `platform_admin` binding: another
platform administrator must do it. Refusals are audited.

**Synopsis**

```text
forge access revoke [OPTIONS] <BINDING>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<binding>` | id | — | yes | The binding's id, from `forge access list` | `<binding-id>` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
# Remove a binding after a prompt
forge access revoke <binding-id>
# Remove it without a prompt, in a script
forge access revoke <binding-id> --yes --non-interactive
```

**Output**

On success the command prints nothing. With `-o json` it prints
`{"result": null, "schema": "forge.cli/v1"}`. Not captured on a live system.

Exit codes:

- `0`: the binding was removed.
- `1`: you answered anything but `yes` at the prompt.
- `2`: `--yes` is missing without a terminal.
- `3`: not signed in.
- `4`: you lack `user.manage` on the binding's scope.
- `6`: no binding with that id.
- `7`: it is the last `platform_admin` binding, or your own `platform_admin`
  binding.

**See also**: [`forge access list`](#forge-access-list),
[Roles and access](/docs/guides/roles-and-access/).

### forge access catalog

Lists every action an access profile can hold, whether it needs an MFA-verified session, and which built-in roles hold it. Use these names with `--action`. Needs `view` on the workspace. With `-o json` each action also has `area`, `label`, `description` and `level` (`read`, `write` or `admin`), the fields the console's role editor uses.

**Synopsis**

```text
forge access catalog
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
forge access catalog
# Only the actions that need MFA
forge -o json access catalog | jq -r '.result[] | select(.mfa) | .action'
```

**Output**

```text
$ forge access catalog
ACTION          MFA    ROLES
view            false  ["platform_admin","workspace_admin","operator","developer","dba","auditor","customer"]
server.manage   false  ["platform_admin","workspace_admin","operator"]
terminal.open   true   ["platform_admin","workspace_admin","operator"]
…
```

Exit codes:

- `0`: success.
- `3`: not signed in.

**See also**: [`forge access profiles create`](#forge-access-profiles-create), [Authorization](/docs/concepts/authorization/).

### forge access profiles list

Lists every access profile with its current actions: the built-in profiles that mirror the roles first, then custom ones. Needs `view` on the workspace.

**Synopsis**

```text
forge access profiles list
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
forge access profiles list
forge -o json access profiles list | jq -r '.result[] | select(.builtin | not) | .name'
```

**Output**

```text
$ forge access profiles list
ID                                    NAME         BUILTIN  REV  ARCHIVED  ACTIONS
01900000-0000-7000-8000-000000000101  Operator     true     1             ["container.manage","deploy",…]
01900000-0000-7000-8000-000000000201  UAT support  false    2             ["server.manage","view"]
```

Exit codes:

- `0`: success.
- `3`: not signed in.

**See also**: [`forge access profiles show`](#forge-access-profiles-show).

### forge access profiles show

Shows one profile with every revision it has had: the action list, who published it and when. Needs `view` on the workspace.

**Synopsis**

```text
forge access profiles show <PROFILE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<profile>` | id or name | — | yes | The access profile, by id or by name | `"UAT support"` |

**Examples**

```sh
forge access profiles show "UAT support"
```

**Output**

```json
{
  "id": "01900000-0000-7000-8000-000000000201",
  "name": "UAT support",
  "builtin": false,
  "revision": 2,
  "actions": ["server.manage", "view"],
  "revisions": [
    { "revision": 1, "actions": ["view"], "created_by": "user:01900000-0000-7000-8000-000000000003", "created_at": 1794590587469 },
    { "revision": 2, "actions": ["server.manage", "view"], "created_by": "user:01900000-0000-7000-8000-000000000003", "created_at": 1794594187469 }
  ]
}
```

Exit codes:

- `0`: success.
- `3`: not signed in.
- `6`: no profile with that id or name.

**See also**: [`forge access profiles publish`](#forge-access-profiles-publish).

### forge access profiles create

Creates a custom profile from an explicit list of actions. A custom profile never gains actions Forge adds later. Needs `access.manage` on the workspace (administrators) and an MFA-verified session.

**Synopsis**

```text
forge access profiles create [OPTIONS] --action <ACTIONS> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | Unique name, 1–80 characters | `"UAT support"` |
| `--action <ACTIONS>` | action, repeatable | — | yes | An action from `forge access catalog`; repeat for each | `--action view --action server.manage` |
| `--description <DESCRIPTION>` | string | empty | no | What the profile is for, up to 500 characters | `--description "UAT on-call"` |

**Examples**

```sh
forge access profiles create "UAT support" --action view --action server.manage --description "UAT on-call"
```

**Output**

Prints the new profile (as `forge access profiles show`, without `revisions`).

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `7`: a profile with that name exists.

**See also**: [`forge access assignments add`](#forge-access-assignments-add).

### forge access profiles publish

Publishes a new revision with exactly the given actions. Every assignment of the profile moves to it at once. You must hold every action the revision adds at every scope where the profile is assigned; otherwise nothing changes and the answer lists those scopes. Built-in and archived profiles can't be published. Needs `access.manage` and an MFA-verified session.

**Synopsis**

```text
forge access profiles publish [OPTIONS] --action <ACTIONS> <PROFILE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<profile>` | id or name | — | yes | The access profile, by id or by name | `"UAT support"` |
| `--action <ACTIONS>` | action, repeatable | — | yes | The complete new action list; repeat for each | `--action view` |
| `--dry-run` | flag | off | no | Show the impact only: added and removed actions, assignments, users, and `blocked` scopes where you lack an added action | `--dry-run` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
# See what would change
forge access profiles publish "UAT support" --action view --dry-run
# Then publish
forge access profiles publish "UAT support" --action view --yes
```

**Output**

```text
$ forge -o json access profiles publish "UAT support" --action view --dry-run
{
  "result": {
    "profile": "01900000-0000-7000-8000-000000000201",
    "from_revision": 2,
    "added": [],
    "removed": ["server.manage"],
    "assignments": 3,
    "users": 2,
    "blocked": []
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `7`: built in, archived, changed meanwhile, or you lack an added action at an assigned scope.
- `6`: no such profile.

**See also**: [`forge access profiles show`](#forge-access-profiles-show).

### forge access profiles duplicate

Copies a profile's current actions into a new, editable profile. Use it to start from a built-in role. The audit log records the source profile and revision. Needs `access.manage` and an MFA-verified session.

**Synopsis**

```text
forge access profiles duplicate <PROFILE> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<profile>` | id or name | — | yes | The access profile, by id or by name | `"UAT support"` |
| `<name>` | string | — | yes | Name of the copy | `"Night operator"` |

**Examples**

```sh
forge access profiles duplicate Operator "Night operator"
```

**Output**

Prints the new profile.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `7`: a profile with that name exists.

**See also**: [`forge access profiles publish`](#forge-access-profiles-publish).

### forge access profiles archive

Archives a custom profile. Its assignments stop granting at once, and it takes no new ones. Its history stays. Built-in profiles can't be archived. Needs `access.manage` and an MFA-verified session.

**Synopsis**

```text
forge access profiles archive [OPTIONS] <PROFILE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<profile>` | id or name | — | yes | The access profile, by id or by name | `"UAT support"` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
forge access profiles archive "UAT support" --yes
```

**Output**

```json
{ "archived": "01900000-0000-7000-8000-000000000201", "assignments_unlinked": 3 }
```

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `1`: you answered anything but `yes` at the prompt.
- `7`: built in, or already archived.

**See also**: [`forge access profiles list`](#forge-access-profiles-list).

### forge access assignments list

Lists assignments at scopes where you hold `user.manage`, your own, and those of groups you are in. Each row has a `USER` or a `GROUP`, never both.

**Synopsis**

```text
forge access assignments list
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
forge access assignments list
```

**Output**

```text
$ forge access assignments list
ID                                    USER                                  GROUP                                 PROFILE                               KIND         SCOPE                                 EXPIRES
01900000-0000-7000-8000-000000000301  01900000-0000-7000-8000-000000000009                                        01900000-0000-7000-8000-000000000201  environment    01900000-0000-7000-8000-0000000000b1
01900000-0000-7000-8000-000000000302                                        01900000-0000-7000-8000-000000000401  01900000-0000-7000-8000-000000000201  service      01900000-0000-7000-8000-0000000000c1
```

Exit codes:

- `0`: success.
- `3`: not signed in.

**See also**: [`forge access assignments add`](#forge-access-assignments-add).

### forge access assignments add

Gives a user, a service account or, with `--group`, every member of a group a profile at one scope: the scope itself and everything under it. You need `user.manage` on the scope, an MFA-verified session, and every action of the profile held over the scope yourself (you can only hand out what you hold).

**Synopsis**

```text
forge access assignments add [OPTIONS] <USER> <PROFILE> <SCOPE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<user>` | username, email or id; with `--group`, group name or id | — | yes | Who gets it; `svc-<name>` for a service account | `alice` |
| `<profile>` | id or name | — | yes | The access profile, by id or by name | `"UAT support"` |
| `<scope>` | `<kind>:<id>` | — | yes | `workspace:<id>`, `cluster:<id or name>`, `project:<id>`, `environment:<id>`, `server:<id>` (node) or `service:<id>`. Only a cluster takes a name | `cluster:uat` |
| `--expires-in-minutes <EXPIRES_IN_MINUTES>` | integer, minutes | never | no | Stop granting after this long, 1–525600. Removed within 30 seconds of expiring | `--expires-in-minutes 480` |
| `--group` | flag | off | no | The first argument names a group; every member gets the profile | `--group` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
forge access assignments add alice "UAT support" cluster:uat --yes
# Temporary access for a night shift
forge access assignments add bob Operator environment:<environment-id> --expires-in-minutes 480 --yes
# Every member of a group, at one project (all its environments)
forge access assignments add uat-oncall "UAT support" project:<project-id> --group --yes
# One service only
forge access assignments add alice "Service restarter" service:<service-id> --yes
```

**Output**

Prints the assignment (`id`, `user_id`, `group_id`, `profile_id`, `scope`, `expires_at`, `created_by`, `created_at`). Exactly one of `user_id` and `group_id` is set; the other is `null`.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `4` with `escalation`: the profile holds an action you don't hold at that scope.
- `7`: the profile is archived.
- `6`: no such user, group, profile or scope.

**See also**: [`forge access assignments remove`](#forge-access-assignments-remove), [`forge access groups`](#forge-access-groups-list).

### forge access assignments remove

Removes an assignment. Access ends at once. Needs `user.manage` on its scope; no MFA, so removing access is never held up.

**Synopsis**

```text
forge access assignments remove [OPTIONS] <ASSIGNMENT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<assignment>` | id | — | yes | From `forge access assignments list` | `<assignment-id>` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
forge access assignments remove <assignment-id> --yes
```

**Output**

Prints nothing on success; with `-o json`, `{"result": null, "schema": "forge.cli/v1"}`.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `6`: no such assignment.

**See also**: [`forge access assignments list`](#forge-access-assignments-list).

### forge access groups list

Lists every group with the ids of its members. Needs `view` on the workspace.

**Synopsis**

```text
forge access groups list
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
forge access groups list
# Groups that alice's id is a member of
forge -o json access groups list | jq -r --arg u <user-id> '.result[] | select(.members | index($u)) | .name'
```

**Output**

```text
$ forge access groups list
ID                                    NAME        MEMBERS                                                                         DESCRIPTION
01900000-0000-7000-8000-000000000401  uat-oncall  ["01900000-0000-7000-8000-000000000009","01900000-0000-7000-8000-00000000000a"]  UAT on-call rota
```

With `-o json` each group is `{id, name, description, created_by, created_at, members}`. Shape from the server's schema; not captured on a live system.

Exit codes:

- `0`: success.
- `3`: not signed in.

**See also**: [`forge access groups create`](#forge-access-groups-create).

### forge access groups create

Creates an empty group. It grants nothing until you assign profiles to it. Needs `access.manage` on the workspace and an MFA-verified session.

**Synopsis**

```text
forge access groups create [OPTIONS] <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | 1–80 characters, unique ignoring case | `uat-oncall` |
| `--description <DESCRIPTION>` | string | empty | no | What the group is for, up to 500 characters | `--description "UAT on-call rota"` |

**Examples**

```sh
forge access groups create uat-oncall --description "UAT on-call rota"
```

**Output**

Prints the new group with `members: []`.

Exit codes:

- `0`: success.
- `2`: the name or description is too long or empty.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `7`: a group with that name exists.

**See also**: [`forge access groups add-member`](#forge-access-groups-add-member), [`forge access assignments add`](#forge-access-assignments-add).

### forge access groups delete

Deletes a group with its memberships and its assignments. Every member loses access through it at once. Needs `access.manage` on the workspace and an MFA-verified session. The command asks for confirmation first.

**Synopsis**

```text
forge access groups delete [OPTIONS] <GROUP>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<group>` | name or id | — | yes | The group | `uat-oncall` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
forge access groups delete uat-oncall --yes
```

**Output**

Prints nothing on success; with `-o json`, `{"result": null, "schema": "forge.cli/v1"}`.

Exit codes:

- `0`: success.
- `1`: you answered anything but `yes` at the prompt.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `6`: no such group.

**See also**: [`forge access groups list`](#forge-access-groups-list).

### forge access groups add-member

Adds a user to a group. The user gets every profile the group holds, at once. Needs `user.manage` on the workspace, an MFA-verified session, and every action the group holds over each scope where it is assigned (you can only hand out what you hold). Adding an existing member changes nothing.

**Synopsis**

```text
forge access groups add-member <GROUP> <USER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<group>` | name or id | — | yes | The group | `uat-oncall` |
| `<user>` | username, email or id | — | yes | The user to add | `alice` |

**Examples**

```sh
forge access groups add-member uat-oncall alice
forge access groups add-member uat-oncall bob@example.com
```

**Output**

Prints nothing on success; with `-o json`, `{"result": null, "schema": "forge.cli/v1"}`.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission), or `escalation`: the group holds actions you don't hold at one of its scopes. The message names the scope and the actions.
- `5`: an MFA-verified session is needed; step up and try again.
- `6`: no such group or user.

:::caution
A member holds everything the group holds, wherever it is assigned. Check the group's assignments with `forge access assignments list` first.
:::

**See also**: [`forge access groups remove-member`](#forge-access-groups-remove-member), [`forge access effective`](#forge-access-effective).

### forge access groups remove-member

Removes a user from a group. Their access through the group ends at once. Needs `user.manage` on the workspace; no MFA, so removing access is never held up. Removing a non-member changes nothing.

**Synopsis**

```text
forge access groups remove-member <GROUP> <USER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<group>` | name or id | — | yes | The group | `uat-oncall` |
| `<user>` | username, email or id | — | yes | The user to remove | `alice` |

**Examples**

```sh
forge access groups remove-member uat-oncall alice
```

**Output**

Prints nothing on success; with `-o json`, `{"result": null, "schema": "forge.cli/v1"}`.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `6`: no such group or user.

**See also**: [`forge access groups add-member`](#forge-access-groups-add-member).

### forge access effective

Shows every action a user holds over a scope (the scope and everything beneath it), from role bindings, their own assignments and their groups, and lists the assignments behind it. MFA-guarded actions are shown as if the user had stepped up. Needs `user.manage` on the scope, unless you ask about yourself.

**Synopsis**

```text
forge access effective [OPTIONS] <USER> <SCOPE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<user>` | username, email or id | — | yes | The user to check | `alice` |
| `<scope>` | `<kind>:<id>` | — | yes | `workspace:<id>`, `cluster:<id or name>`, `project:<id>`, `environment:<id>`, `server:<id>` or `service:<id>` | `service:<service-id>` |
| `--allowed` | flag | off | no | List only the allowed actions | `--allowed` |

**Examples**

```sh
# Everything alice may do on one service
forge access effective alice service:<service-id> --allowed
# Which assignments give her access there
forge -o json access effective alice service:<service-id> | jq '.result.sources'
```

**Output**

```json
$ forge -o json access effective alice service:<service-id> --allowed
{
  "result": {
    "user_id": "<user-id>",
    "scope": { "kind": "service", "id": "<service-id>" },
    "actions": [
      { "action": "view", "allowed": true },
      { "action": "job.create", "allowed": true },
      { "action": "deploy", "allowed": true }
    ],
    "sources": [
      {
        "assignment_id": "<assignment-id>",
        "profile_id": "<profile-id>",
        "profile_name": "Service restarter",
        "scope": { "kind": "service", "id": "<service-id>" },
        "via": "direct",
        "group_id": null,
        "group_name": null
      }
    ]
  },
  "schema": "forge.cli/v1"
}
```

`via` is `direct` or `group`. `sources` lists access profile assignments on the scope or above it; role bindings count in `actions` but are not listed. Shape from the server's schema; not captured on a live system.

Exit codes:

- `0`: success.
- `2`: a malformed scope or a scope kind other than the six above.
- `3`: not signed in.
- `4`: you lack `user.manage` on the scope.
- `6`: no such user or scope.

**See also**: [Held over a scope](/docs/concepts/authorization/#held-over-a-scope), [`forge access assignments list`](#forge-access-assignments-list).
