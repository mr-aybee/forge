---
title: forge sessions
description: The forge sessions commands that list open terminal and exec sessions and end one.
sidebar:
  order: 25
---

`forge sessions` lists the terminal and exec sessions that are open right
now, from the console and the CLI, and ends one on demand. For how sessions
open and stay safe, see [Browser terminal (SSH)](/docs/guides/terminal/). Every
command takes the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge sessions list`](#forge-sessions-list) | Lists your open sessions, or everyone's with `--all` |
| [`forge sessions revoke`](#forge-sessions-revoke) | Ends an open session within a second |

### forge sessions list

Lists your open terminal and exec sessions, newest first. Any signed-in user
can list their own. `--all` lists everyone's and needs `user.manage` on the
platform, for example **Platform admin**.

**Synopsis**

```text
forge sessions list [--all]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--all` | flag | off | no | Lists the sessions of every user, not only yours | `--all` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row (this command's `--all` means everyone's, not every row) | `--limit 0` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |

The list holds sessions that are open at this moment. Ended sessions are not
listed; their start and end are in the audit log (`terminal.open`,
`terminal.close`, `terminal.exec`, `terminal.exec_close`).

**Examples**

```sh
# Your open sessions
forge sessions list

# Everyone's open sessions
forge sessions list --all

# Ids of your open exec sessions
forge -o json sessions list | jq -r '.result[] | select(.kind == "exec") | .id'
```

**Output**

Captured with no open session:

```text
ID  USER  NODE  LOGIN  KIND  CLIENT  STARTED
```

```json
{
  "result": [],
  "schema": "forge.cli/v1"
}
```

Each session has these fields. The table shows all of them except
`user_id`; `NODE` is the node id and `STARTED` is `started_at`:

| Field | Meaning |
| --- | --- |
| `id` | The session id, 32 hex characters. `forge nodes exec -o json` prints it as `session` |
| `user_id`, `user` | Who opened it: the user id, and the user name or email |
| `server_id` | The node's id |
| `login` | The OS account on the node |
| `kind` | `terminal` or `exec` |
| `client` | `console` or `cli` |
| `started_at` | When it opened, in epoch milliseconds |

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: `--all` without `user.manage` on the platform.

**See also**: [`forge sessions revoke`](#forge-sessions-revoke),
[`forge nodes terminal`](/docs/reference/cli/nodes/#forge-nodes-terminal),
[`forge nodes exec`](/docs/reference/cli/nodes/#forge-nodes-exec).

### forge sessions revoke

Ends an open session within a second. You can end your own sessions; ending
someone else's needs `user.manage` on the platform. Audited as
`terminal.revoke`. The CLI asks you to type `yes` unless you pass `--yes`.

**Synopsis**

```text
forge sessions revoke [--yes] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | id | — | yes | The session id from [`forge sessions list`](#forge-sessions-list) | `365286e7038834c998d3eec380bb1dc2` |
| `--yes` | flag | off | no | Skips the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

The user whose session ends sees it close: `forge nodes terminal` and
`forge nodes exec` exit with `1` and the code `revoked`.

**Examples**

```sh
# End one of your sessions after a confirmation prompt
forge sessions revoke <session-id>

# End a session without a prompt
forge sessions revoke <session-id> --yes
```

**Output**

`revoked` and the session as it was listed. Shape from the server's schema;
not captured on a live system. The default output prints the same object
without the wrapper:

```json
{
  "result": {
    "revoked": true,
    "session": {
      "id": "<session-id>",
      "user_id": "<user-id>",
      "user": "<user-name>",
      "server_id": "<node-id>",
      "login": "<login>",
      "kind": "terminal",
      "client": "cli",
      "started_at": 1794590525506
    }
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: the session is ending.
- `1`: you did not type `yes` at the prompt (`cancelled`).
- `2`: no `--yes` in non-interactive use.
- `3`: not signed in.
- `6`: no open session with this id. Someone else's session that you may not
  end gives the same answer, so its existence is not revealed.

**See also**: [`forge sessions list`](#forge-sessions-list),
[Browser terminal (SSH)](/docs/guides/terminal/#open-a-session-from-the-command-line).
