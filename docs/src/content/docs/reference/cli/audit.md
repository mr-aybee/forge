---
title: forge audit
description: Read and filter the audit log and verify its hash chain and signatures from the command line.
sidebar:
  order: 7
---

`forge audit` reads Forge's signed, hash-chained audit log and checks that
it is intact. For how the log works, read [Audit log](/docs/concepts/audit/). The
commands use the [audit endpoints](/docs/reference/rest-api/#audit). Every command
takes the [global flags](/docs/reference/cli/#global-flags).

## Permissions

| Commands | Cedar action | Scope | MFA |
| --- | --- | --- | --- |
| `list` | `audit.read` | the platform | no |
| `list --workspace <ID>` | `audit.read` | that workspace | no |
| `verify` | `audit.read` | the platform | no |

`audit.read` is in the `platform_admin`, `workspace_admin` and `auditor`
roles. Without `--workspace`, `list` reads the whole log and needs a grant
at platform scope; `verify` always does. An auditor granted on one workspace
only passes `--workspace` with that workspace's id. See the [Cedar schema](/docs/reference/cedar-schema/).

## Commands

| Command | What it does |
| --- | --- |
| [`forge audit list`](#forge-audit-list) | Lists audit entries, newest first, with optional filters |
| [`forge audit verify`](#forge-audit-verify) | Verifies the hash chain and signatures |

### forge audit list

Lists audit entries, newest first. Needs `audit.read` on the platform, or
on the workspace in `--workspace`.

**Synopsis**

```text
forge audit list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--action <ACTION>` | string, prefix | none | no | Only actions that start with this | `--action stack.` |
| `--actor <ACTOR>` | string, substring | none | no | Only actors that contain this, such as `user:<id>` or `system:forge` | `--actor user:<user-id>` |
| `--outcome <OUTCOME>` | enum: `success`, `denied`, `failure` | none | no | Only entries with this outcome | `--outcome denied` |
| `--resource <RESOURCE>` | string, substring | none | no | Only resources that contain this, such as `server:<id>` | `--resource server:<node-id>` |
| `--since <SINCE>` | duration (`90s`, `30m`, `12h`, `7d`) or epoch milliseconds | none | no | Only entries at or after this time. A duration counts back from now | `--since 1d` |
| `--workspace <WORKSPACE>` | workspace id | none (all entries) | no | Only this workspace's entries. Needs `audit.read` on that workspace instead of the platform | `--workspace <workspace-id>` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |
| `--before <SEQ>` | integer (sequence number) | none | no | Only entries older than this one: a cursor, the `SEQ` of the last row you have | `--before 5120` |
| `--until <UNTIL>` | duration (`90s`, `30m`, `12h`, `7d`) or epoch milliseconds | none | no | Only entries before this time. A duration counts back from now | `--until 7d` |

:::note
With any filter, the server searches only the newest 10,000 entries, so
`list` never returns an entry older than those. Older entries stay in the
database; `list` does not reach them.
:::

**Examples**

```sh
# The 20 newest entries
forge audit list --limit 20
# Denied actions of the last day
forge audit list --outcome denied --since 1d
# Terminal sessions on one node in the last 12 hours
forge audit list --action terminal. --resource server:<node-id> --since 12h
# An auditor granted on one workspace: that workspace's denied actions
forge audit list --workspace <workspace-id> --outcome denied --since 7d
# Everything one user changed on stacks, as JSON
forge -o json audit list --actor user:<user-id> --action stack. > stack-changes.json
```

**Output**

```text
SEQ   AT             ACTOR                                      ACTION           OUTCOME  RESOURCE
1239  1794590547295  user:01900000-0000-7000-8000-000000000003  file.acl         success  server:01900000-0000-7000-8000-000000000004
1238  1794590546214  user:01900000-0000-7000-8000-000000000003  file.acl         success  server:01900000-0000-7000-8000-000000000004
1237  1794590545109  user:01900000-0000-7000-8000-000000000003  file.trash.list  success  server:01900000-0000-7000-8000-000000000004
1236  1794590542558  user:01900000-0000-7000-8000-000000000003  file.trash.list  success  server:01900000-0000-7000-8000-000000000004
1235  1794590540926  user:01900000-0000-7000-8000-000000000003  file.search      success  server:01900000-0000-7000-8000-000000000004
```

`AT` is epoch milliseconds. With `-o json`, each entry also has its
`details`, hashes and signature:

```json
{
  "result": [
    {
      "action": "terminal.exec_close",
      "actor": "user:01900000-0000-7000-8000-000000000003",
      "at": 1794590528986,
      "details": {
        "bytes_in": 0,
        "bytes_out": 8,
        "client": "cli",
        "command": "sh -c 'echo out; echo err >&2; exit 3'",
        "duration_secs": 0,
        "exit_status": 3,
        "host": "192.0.2.11",
        "login": "root",
        "port": 22,
        "reason": "exited",
        "session": "4bc57e95d8ebe92b1b503d1eab406ee2"
      },
      "hash": "b2aa62c4a0e1133fb595a87ad7b318cff187de7f3b2cb6f5539787517accb81c",
      "id": "01900000-0000-7000-8000-000000000015",
      "outcome": "success",
      "prev_hash": "465c44c5491afbd346a83d00e4496639602db5da052adbb6ef161796ea7d9981",
      "resource": "server:01900000-0000-7000-8000-000000000004",
      "seq": 1225,
      "signature": "-u13MprxrfMn3eXRtbNWNGFoBqa142JkTreocLFBG2KQOWZKddFMbODHQQ-fFBiEl8_RYjZ3PAxIyAb8HbrIDA",
      "workspace_id": "01900000-0000-7000-8000-000000000001"
    },
    …
  ],
  "schema": "forge.cli/v1"
}
```

| Field | Description |
| --- | --- |
| `seq` | Position in the chain, starting at `1` |
| `at` | Time of the entry, epoch milliseconds |
| `actor` | Who acted: `user:<id>`, `system:forge`, … |
| `action` | What was done, such as `stack.deploy` or `terminal.exec` |
| `resource` | What it was done to, such as `server:<id>` |
| `outcome` | `success`, `denied` or `failure` |
| `details` | Action-specific data. Never holds passwords, codes or key material |
| `workspace_id` | The organization, or `null` for platform-wide entries |
| `prev_hash`, `hash` | The previous entry's hash and this entry's hash |
| `signature` | Ed25519 signature of `hash` with the control plane's audit key |

Exit codes: `0` success; `2` an invalid `--since` or `--workspace` id; `3`
not signed in; `4` no `audit.read` on the platform (or on the workspace in
`--workspace`).

**See also**: [`forge audit verify`](#forge-audit-verify),
[Audit log](/docs/concepts/audit/).

### forge audit verify

Walks the whole chain from the first entry and checks every hash, link,
sequence number and signature. Needs `audit.read` on the platform.

**Synopsis**

```text
forge audit verify [OPTIONS]
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Check the log
forge audit verify
# In a scheduled check: alert when the chain is broken
forge -o json audit verify > audit-verify.json || echo "audit chain broken" >&2
```

**Output**

The table output prints the report as a key/value list. With `-o json`:

```json
{
  "result": {
    "entries_checked": 1239,
    "first_invalid": null,
    "head_hash": "373a8ccf652f7e168745850d1b3554fde4b96a7d5d4a2ac3bac4b0a3659965ef",
    "ok": true
  },
  "schema": "forge.cli/v1"
}
```

| Field | Description |
| --- | --- |
| `ok` | `true` when the whole chain is intact |
| `entries_checked` | Entries verified |
| `head_hash` | Hash of the newest entry |
| `first_invalid` | `null`, or `[<seq>, "<reason>"]` for the first entry that fails |

:::caution
A broken chain means an entry was changed, removed or inserted outside Forge.
Keep the report and the database as they are, and investigate before you
restore anything.
:::

Exit codes: `0` the chain is intact; `1` the chain is broken (code
`audit_broken`; the report is printed first); `3` not signed in; `4` no
`audit.read` on the platform.

**See also**: [`forge audit list`](#forge-audit-list),
[Audit log](/docs/concepts/audit/).
