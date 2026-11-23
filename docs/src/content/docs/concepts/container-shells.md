---
title: Container shells
description: How a shell inside a container reaches your browser through the node's agent, without an SSH account.
sidebar:
  order: 14
---

This page explains how a shell inside a container gets from the node to your
browser or the `forge` CLI. You need no terminal account or credential on the
node, and the console uses the same terminal pane as the node terminal. To
open one, see [Containers](/docs/guides/containers/).

## The path

```diagram
alt: Container shell connection path
flow:
  steps:
    - { label: browser / forge CLI, edge: WebSocket }
    - { label: control plane, tone: accent, edge: "HTTP/2 stream (dialled by the node)", dir: back }
    - { label: agent, edge: Unix socket }
    - { label: root helper, edge: Docker API }
    - { label: exec (TTY) }
```

1. You open **Shell** on a container page. The API authorizes
   `container.exec` on the node, then checks three things: the node runs
   Linux, it is online, and its agent reports that it supports container shells
   (`container_exec` in its facts). An agent reports that only when its helper
   has pinned Forge's command key. The API also holds one of your eight places
   and one of the node's eight places until the shell is registered, so
   parallel opens cannot pass the limits. Every refusal is audited as
   `container.exec` with the reason.
2. Forge signs a `ContainerExecOpen` command for that node. It holds the
   container, the terminal size, a session id and a single-use stream token.
   Forge keeps only the token's hash.
3. The agent verifies the signature, checks its local allow-list (the
   command has its own entry, `container_exec`) and its limit of eight
   shells, and forwards the command as Forge signed it to the root helper.
4. The helper verifies the signature again with the key it pinned itself,
   and refuses a replayed or expired command. A helper without a pin refuses
   container shells altogether. It then asks the Docker Engine for an exec
   with a terminal, as the image's default user, with
   `TERM=xterm-256color`, running `bash` when the image has it and `sh`
   otherwise. It sizes the terminal and answers the agent.
5. The agent dials Forge's agent port with its own certificate and posts a
   stream to `/agent/v1/exec/<session>` with the token in a header. Forge
   accepts it only from the node it signed the command for, and only once.
6. Bytes flow in small frames: keystrokes, output, resizes, and at the end
   the shell's exit code. The agent passes the frames on without reading
   them.

## Limits and endings

| Limit | Value |
| --- | --- |
| Shells per node | 8 (enforced by the helper, checked early by the agent and Forge) |
| Shells per user | 8 |
| Time for the node to answer | 15 seconds, then `the agent did not answer: update the agent` |
| Idle time | 30 minutes without input (Forge); 31 minutes without traffic either way (the helper) |
| Lifetime | 12 hours, enforced by Forge and by the helper |
| Access re-check | every 30 seconds (sign-in still valid, `container.exec` still held) |

A shell ends when it exits, when you close it, when someone revokes it under
**Remote sessions**, or at a limit above. Ending it closes the exec's
terminal, which hangs up the shell. Docker has no call that kills an exec,
so a program that ignores the hang-up keeps running inside the container.

A slow connection slows the shell, not the server: every hop has a small,
bounded buffer, so output waits in the container until your browser reads it.

## Security

- `container.exec` on a node opens a shell in **any** container on that
  node, including containers Forge did not create. The SSH path behaves the
  same. Scope the permission by node, not by container.
- The helper does not rely on the agent for limits: it checks the signed
  expiry and ends a shell at the signed idle and lifetime limits itself.
- Existing agents opt in: an update never adds `container_exec` to
  `allowed_commands`. See
  [Configuration](/docs/reference/configuration/#agent-agenttoml).
- No SSH account, password or `sudo` rule on the node is involved.
- The agent never gets the Docker socket. Only the root helper uses it, and
  only for a command Forge signed for this node.
- The token is single-use, travels in a header and never in a URL.
- Every shell is audited as `container.exec` when it opens and
  `container.exec_close` when it ends, with `path: agent`, the duration,
  bytes in each direction, the reason and the exit code.

:::caution
A shell inside a container runs as the image's user, often root. A container
that is privileged, mounts the Docker socket or mounts host folders writable
is a path to root on the node. Grant `container.exec` only to people you
would give that shell to.
:::

## The SSH path

Before agent-path shells, Forge opened a container shell over SSH as the
node's terminal account and ran `sudo docker exec`. That path remains, but
only when you ask for it with `login=` on the REST API. Windows nodes support
neither path.

## Related

- [Containers](/docs/guides/containers/)
- [Port tunnels](/docs/concepts/port-tunnels/), which use the same dial-back stream
- [REST API: terminal](/docs/reference/rest-api/#terminal-browser-ssh)
