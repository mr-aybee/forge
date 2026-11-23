---
title: Open a port tunnel
description: Reach a service, a stack service, a database or SSH on a node from your computer with forge tunnel, and give someone access with a grant.
sidebar:
  order: 23
---

A port tunnel connects a port on your computer to a service on a node, so
you can run `psql`, a browser or `ssh` against something that only listens
on the node itself. You name the **service**; Forge finds the node, the port
and a free local port for you. For how it works, see
[Port tunnels](/docs/concepts/port-tunnels/).

## Before you begin

- A role on the service's environment: **Developer**, **Operator**,
  **Workspace admin** or **Platform admin** hold `network.tunnel` there. Or
  an explicit [grant](#give-someone-access-with-a-grant) from an
  administrator. SSH tunnels need `terminal.open` on the node, like the
  browser terminal.
- When the server requires MFA, a sign-in with a second factor. With an API
  key, tunnels work only when the server does not require MFA (the default).
- The [forge CLI](/docs/reference/cli/), signed in.
- The node is online. Linux and Windows nodes both work.

## 1. Find the service's ports

Open the service in the console. **Workloads → Services →** *app*,
**Workloads → Stacks →** *stack* and **Packages → App Store →** *package*
each show a **Ports** block: port, role, the nodes it runs on and the exact
`forge tunnel open` line. Select the line to copy it.

The console lists and closes tunnels but cannot open one: a browser cannot
listen on a port of your computer.

## 2. Open the tunnel

```sh
forge tunnel open --app site                          # a service; picks its first healthy node
forge tunnel open --stack seq-b --service seq         # a compose service; asks which port when it has several
forge tunnel open --db orders-pg                      # a database installed from the App Store
forge tunnel open --node lab-b --ssh                  # SSH on a node (the node's configured SSH port)
```

Add `--node <node>` to pick a node when the service runs on several, `-l
<port>` to choose the local port, and `--for 30m|8h|3d` to set the
lifetime. The default lifetime is 8 hours and the longest 7 days. The CLI
prints the local address and keeps running:

```
→ lab-b:8080 → http://127.0.0.1:18080 (expires in 8 h)
```

Leave it open. The local port is **remembered** per target, node and remote
port in the CLI context file, so the next `forge tunnel open --app site`
uses `18080` again and your bookmarks and connection strings keep working.

```sh
forge tunnel ports                   # the remembered local ports
forge tunnel ports forget --app site # forget one
```

:::note
Administrators can also tunnel to a raw port with `forge tunnel open --node
<node> --port 9000`. It needs `network.tunnel.any`, bypasses the service
catalogue and is audited as a raw tunnel. Prefer a service target.
:::

### Go straight to the node with `--direct`

By default every byte travels through the Forge server. When your machine
can reach the node's SSH port, add `--direct` and the CLI connects to the
node's SSH instead:

```sh
forge tunnel open --app web-html --node lab-c -l 18081 --direct
```

```
→ lab-c:8081  →  http://127.0.0.1:18081   (direct SSH, expires in 8 h, Ctrl-C to close)
```

For each connection, the CLI asks Forge for an SSH certificate that is
valid for one minute. Forge checks your access again every time. The
certificate logs in as the `forge-tunnel` account, which has no shell, and
the node lets it forward only to this target's address and port. The CLI
accepts only the host key pinned for the node.

`--direct` needs:

- the node's SSH endpoint set, with its host key pinned, on the node's
  **Terminal** page;
- a Linux node with the browser terminal enabled (it trusts Forge's SSH CA)
  and a current agent;
- the node's SSH port reachable from your machine.

:::caution
A direct connection does not pass through the server. Closing the tunnel,
revoking access or an expired lifetime stops new connections and makes the
CLI drop the open ones. A modified client could keep an already-open
connection until it closes. The server does not count bytes for direct
tunnels.
:::

## 3. Connect through it

In another terminal, use the local address the CLI printed:

```sh
psql -h 127.0.0.1 -p 15432 -U postgres        # a database
ssh -p 2222 forge-ops@127.0.0.1               # an SSH tunnel
```

## 4. Close it

Press **Ctrl-C** in the terminal running `forge tunnel open`. From
elsewhere:

```sh
forge tunnel list
forge tunnel close <tunnel-id>
```

In the console, **Profile → Tunnels** lists your open tunnels with a
**Close** button; **Administration → Tunnels** lists everyone's.

## Give someone access with a grant

A grant lets one person tunnel to one target without a role on its
environment, for a limited time. It needs `access.manage` (**Workspace admin**
or **Platform admin**).

1. Open **Administration → Tunnels → Grants** and select **Create grant**.
2. Pick the **User** and the **Target**: a service, a stack service, a
   database package, SSH on a node, or (administrators only) raw ports on a
   node. Leave **Node** empty to cover every node the service runs on.
3. Choose the **Lifetime**: 30 minutes to 7 days. **Never expires** is
   available only when the server sets `[tunnels] allow_never_expiry =
   true` and you are an administrator; otherwise the option says why it is
   disabled.
4. Select **Create grant**.

Or from the CLI:

```sh
forge tunnel grants create --user alice --app site --for 1h
forge tunnel grants list
forge tunnel grants revoke <grant-id>
```

**Revoke** on a grant closes the tunnels open under it within 30 seconds.
Tunnels opened under a grant end when the grant expires.

## Verify

- `forge tunnel list` shows the tunnel with its target, node, local address,
  expiry and bytes.
- **Profile → Tunnels** lists it; **Administration → Tunnels → Open** shows
  it with your name. Closed tunnels move to **History** with the reason.
- The audit log has `network.tunnel.open`, and `network.tunnel.close` after
  you close it. Grants log `network.tunnel.grant.create` and
  `network.tunnel.grant.revoke`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `no role or grant allows a tunnel to <service>` | You hold no role on the service's environment and have no grant | Ask for **Developer** or **Operator** on the environment, or for a grant under **Administration → Tunnels → Grants** |
| `raw ports need network.tunnel.any` | `--port` without a service is for administrators | Use `--app`, `--stack --service`, `--db` or `--ssh`, or ask an administrator |
| `<service> listens on several ports` without a prompt | No terminal to ask on | Pass `--port <n>` or `--all` |
| `step_up_required` or `mfa_required` | `network.tunnel` needs a verified second factor | Sign in with your authenticator or passkey (`forge auth login`); API keys work only where MFA is off |
| `cannot listen on 127.0.0.1:18080` | The remembered local port is in use | Pass `-l <port>` once; the CLI remembers the new one |
| `the node refused the connection: nothing answers on port 8080` | The service is not running on that node | Start it, or pick another node with `--node` |
| `never needs [tunnels] allow_never_expiry` | Never-expiring tunnels and grants are off on this server | Use a lifetime up to 7 days, or ask the operator to enable the setting |
| `you already have 8 open tunnels` | The per-user limit | Close one, or ask an administrator to raise `[tunnels] max_per_user` |
| The tunnel closes by itself | 15 minutes without traffic, its lifetime ended, its grant was revoked, your sign-in ended, the server restarted or an administrator closed it | **Administration → Tunnels → History** names the reason; open it again |
| `--bind … lets other machines use the tunnel` | `--bind` is not a loopback address | Add `--allow-remote-bind` if you mean it |
| `this node is not set up for direct tunnels` | The node does not trust Forge's SSH CA, or its agent predates direct tunnels | Enable the browser terminal on the node and update its agent (`forge agents update`), or open the tunnel without `--direct` |
| `this node has no SSH endpoint with a pinned host key` | No SSH endpoint was saved for the node | Set it on the node's **Terminal** page, then retry |
| `SSH to <host>:22 timed out` | Your machine cannot reach the node's SSH port | Open the tunnel without `--direct` (it relays through the server) |
| `the node's SSH host key is not the pinned …` | Something other than the node answered, or the node's key changed | Do not continue; check the node, then re-pin its key on the **Terminal** page |

## Next steps

- [Port tunnels](/docs/concepts/port-tunnels/): the path, limits and revocation.
- [`forge tunnel`](/docs/reference/cli/tunnel/): every flag.
- [Open a terminal](/docs/guides/terminal/): the browser terminal for the same nodes.
