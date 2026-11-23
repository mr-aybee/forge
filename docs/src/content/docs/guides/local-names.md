---
title: Rename a node and reach services by name
description: Give a node a DNS name, check that every node resolves it, and find the address and port of a service.
sidebar:
  order: 21
---

Every node resolves the names of your other nodes and services, such as
`db.local` or `api.shop.prod.local`. This guide shows how to set a node's
name and how to reach a service by name. For how it works, see
[Local names](/docs/concepts/local-names/).

## Before you begin

- You need `server.rename` on the node to rename it (**Operator**,
  **Workspace admin** or **Platform admin**), and `view` to list names.
- Linux nodes (Debian, Ubuntu, RHEL-likes) and Windows nodes are supported.
  Windows nodes get the names in their hosts file but are not tested on every
  release.
- To change the suffix, you need `system.update` on the platform
  (**Platform admin**) and an MFA-verified session.
- The node's agent allows the `names` command (it does by default; see
  [`allowed_commands`](/docs/reference/configuration/#agent-agenttoml)).

## Rename a node

1. Open **Nodes**, select the node and choose **Rename**.
2. In **DNS name**, enter a label: lowercase letters, digits and `-`, up
   to 63 characters, for example `db`.
3. Select **Save**.

Or with the CLI:

```sh
forge nodes dns-name <node> db
```

Leave **DNS name** empty (or pass `""`) to go back to the name derived
from the node's console name or hostname.

:::note
A DNS name another node already has is refused with
`409 Conflict`. Rename that node first.
:::

## Reach a service by name

1. List the names and ports:

   ```sh
   forge dns list --kind service
   ```

2. Connect from any node, or from a Forge container on any node, using the
   name and the port from the list:

   ```sh
   curl http://api.shop.prod.local:8080/health
   ```

## Change the suffix

Every name ends in the same suffix, `local` by default. To use another one,
such as `home.arpa`:

1. Open **DNS**. In **Local names**, enter the new suffix in **Local name
   suffix**. The preview under the field shows how names will look.
2. Select **Save**, read the warning and select **Change suffix**.

Or with the CLI:

```sh
forge dns suffix set home.arpa
```

To go back to the suffix in `forge.toml`, select **Reset** or run
`forge dns suffix reset`.

:::caution
All names change on every node within one sync pass (30 seconds). The old
names stop resolving at once. Forge containers follow on their next sync.
Update anything that still uses the old names.
:::

## Verify

- On any node, `getent hosts db.local` prints the node's address.
- The console's **DNS** page lists the name under **Local names**, and no
  node shows **not updated**.
- `forge dns list` shows the new name within 30 seconds.
- After a suffix change, `forge dns suffix get` shows the new suffix with
  source `setting`, and `getent hosts <node>.<suffix>` resolves on any node.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **not updated** next to a node; `forge dns list` says the command is not allowed | `names` was removed from the node's `allowed_commands` | Add `"names"` to `allowed_commands` in `agent.toml` and restart the agent |
| `getent hosts` finds the name but a browser does not | The browser or program asks mDNS directly for `.local` | [Change the suffix](#change-the-suffix) to `home.arpa` or test with `curl`. See [`.local` and mDNS](/docs/concepts/local-names/#local-and-mdns) |
| Saving the suffix fails with `step_up_required` | Your session is not MFA-verified | Verify with your authenticator and try again |
| The suffix is refused as a public top-level domain | One label such as `com` or `io` could shadow internet names | Use `home.arpa`, `internal` or a domain you own |
| **Local name suffix** is read-only | You are not a platform admin | Ask a platform admin to change it |
| Old names still resolve on a node | That node has not received the new list yet (offline, or **not updated**) | Bring the node online; it gets the list on its next heartbeat |
| A node's name ends in `-` and six characters | Another node had that label when this node got its name | Set a DNS name on the node |
| A console rename did not change the local name | Local names are kept once assigned | Set the **DNS name** in **Rename node** or with `forge nodes dns-name` |
| The hosts file is empty or partial after a crash | The node stopped while Forge was writing the file | Restore your own lines from a copy; Forge writes its block again on the next sync |
| The error says the hosts file is over the size limit | `/etc/hosts` is larger than 16 MiB (4 MiB for a container) | Remove stale entries from the file; Forge does not cut it |
| A container does not resolve names | It is not a Forge container (no `forge.app` label), or it restarted less than 10 minutes ago | Deploy it through Forge, or wait for the next sync |

## Next steps

- [Local names](/docs/concepts/local-names/): addresses, sync and the `.local`
  trade-offs.
- [`[names]` settings](/docs/reference/configuration/#local-names).
- [Cluster network](/docs/guides/cluster-network/): mesh addresses between
  nodes of a cluster.
