---
title: Containers
description: Manage Docker containers and images on a node from the console.
sidebar:
  order: 5
---

This guide shows you how to start, stop and inspect Docker containers on a
node, read their logs and pull images. To run new containers, see
[Create a container](/docs/guides/create-container/) and
[Deploy a compose stack](/docs/guides/compose-stacks/).

## Before you begin

- The node runs Linux with **Docker Engine** installed from the
  [App Store](/docs/guides/app-store/). Container management is not supported on
  Windows nodes.
| Action | Needs |
| --- | --- |
| List containers and images, open a container, read logs, sample usage | `server.manage` on the node |
| Start, stop, restart, remove, pull | `container.manage` on the node |
| Open a shell inside a container | `container.exec` on the node |
| Reveal environment values | `container.inspect_env` on the node, with an MFA-verified session |

All four are held by **Operator**, **Admin** and **Platform admin**. The
shell needs no terminal account: it runs through the node's agent, which must
be current (it reports `container_exec`). Update an older agent from
**Packages → Updates → Agents**.

## Steps

1. Open the node and select the **Docker** tab, then **Containers**. The
   **Containers** panel has two views: **containers** and **stacks**. Images,
   volumes, networks, events and disk use have their own sub-tabs; see
   [Docker images, volumes and networks](/docs/guides/docker-resources/). The cluster's
   **Containers** page in the sidebar lists the containers of every Docker
   node in the cluster, grouped by node. Each node's header shows how many
   of its containers run; a node with Docker but no containers shows
   **No containers**. While you filter, only nodes with a match are shown.
2. In **containers**, find the container (use **Filter containers** for
   many). Open its row menu to **Start**, **Stop**, **Restart** or
   **Remove** it. The **CPU / memory** column takes a sample on request.
3. Select the container's name to open its page. It has four tabs:

   | Tab | What you see |
   | --- | --- |
   | **Overview** | **Resource history** charts (CPU, Memory, Network ↓ in / ↑ out) with a **1 h**, **6 h**, **24 h** or **7 d** window: per minute up to two days, hourly beyond, with the restart count when it is not zero. A summary strip with state (uptime, or exit code and stop time), health, image and digest, node, the owning service or stack as a link, and restart policy. Below it: live CPU and memory, entrypoint and command (`image default` when not overridden), created and started times, ports (host → container and protocol, one row per mapping with its IPv4 and IPv6 bind addresses as chips, with an **open** link for published TCP ports), networks with the container's IP, mounts (type, source → target, `ro`/`rw`) and labels (Forge's own `forge.*` labels first and highlighted), which you can collapse and filter |
   | **Logs** | The container's stored output, from the analytics database, with a filter, **Open in Logs** (the **Logs** page filtered to this container), **Copy** and **Load older lines**. Turn on **Live** to follow the node's output directly instead: pick how many lines to load (100, 500, 2,000 or 5,000), and new lines are added every two seconds, also while the container is stopped, started or restarted. **No stored logs** means collection is off or excludes the container (node **Details → Container telemetry**). See [Container log collection](/docs/concepts/container-logs/) |
   | **Shell** | **Open shell** starts `bash` (or `sh`) inside the running container as the image's default user, in the terminal dock. Resizing the dock resizes the shell; when the shell ends, the dock shows its exit code. The session stays open while you navigate and can pop out like a node terminal |
   | **Environment** | The names of the container's environment variables, with a name filter and, when Forge sealed secrets for the service, **All** / **Hide sealed** / **Only sealed**. **Reveal values** shows every value at once after an MFA check; **Hide values** masks them again. Values are cleared when you leave the tab. Secrets that Forge sealed for its own services stay `<sealed>`. Without `container.inspect_env` the button is disabled and says why |

   Containers that Forge created for a service are named
   `forge-app-<environment>-<service>`; the **Overview** tab links to that
   service, and the service's **Nodes** table links back to the container.
4. To pull an image, open **Docker → Images** and select **Pull image**. See
   [Docker images, volumes and networks](/docs/guides/docker-resources/).

Container packages from the App Store, such as SQL Server and RustFS, appear
here too. **stacks** lists the compose stacks on the node. **Create** offers
**Create container** and **Create from Compose**.

:::caution
A shell inside a container is a shell as the container's user, usually root.
In a container that runs privileged, mounts the Docker socket or mounts host
folders writable, that is a path to root on the node. `container.exec` is
not MFA-guarded, so grant **Operator** on a node only to people you would
give that shell to. Every shell is audited as `container.exec`.
:::

## How it works

Access to `/var/run/docker.sock` is root-equivalent, so the agent never gets
it (it is not added to the `docker` group). The root `forge-agent-helper`
talks to the Docker Engine API and exposes a closed set of operations:
`info`, `list`, `images`, `inspect`, `logs`, `stats`, `start`, `stop`,
`restart`, `remove`, `pull`. There is no arbitrary `run`. Container
ids and image references are validated by the API and again by the helper.
`inspect` returns environment variable names only, since values often hold
secrets; **Reveal values** asks for them explicitly, needs
`container.inspect_env` with MFA, is audited as `container.env.reveal`, and
the values are not kept with the job.

The shell goes through the agent and the helper too. Forge signs a request
for that one container; the helper checks the signature itself, refuses a
replayed request, and starts a Docker exec with a terminal (`bash`, or `sh`
when the image has no bash). The agent connects back to Forge and relays the
bytes; no SSH account or password is involved. A node runs at most eight
container shells, and you can hold at most eight at once. Ending the session
(closing the tab, revoking it under **Remote sessions**, or the idle or
12-hour limit) closes the shell's terminal; Docker has no way to kill an exec,
so a program that ignores the hang-up keeps running in the container. See
[Container shells](/docs/concepts/container-shells/) for the full path.

The REST API and older setups can still take the SSH path: pass `login=` and
Forge connects as that terminal account and runs `sudo docker exec`. See the
[REST reference](/docs/reference/rest-api/#terminal-browser-ssh).

Every operation is an `agent.command` job and is audited. The API waits up
to 25 seconds for a result.

## Verify

- After **Start** or **Stop**, the **State** column shows the new state.
- After a pull, the image is listed in **Docker → Images**, and the job in
  **Operations → Jobs** shows `succeeded`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **Create** is disabled with "Install Docker Engine on this node first" | Docker is not installed | Install **Docker Engine** from the App Store |
| Row actions are refused with `403` | You lack `container.manage` | Ask for **Operator** on the node or its cluster |
| A pull seems to do nothing | Pulls run in the background | Follow the job in **Operations → Jobs** |
| "Another container action is running" | One container action runs at a time | Wait for it to finish |
| **Open shell** fails with `403` | You lack `container.exec` | Ask for **Operator** on the node or its cluster |
| **Open shell** fails with `container shells need a current agent` (`agent_update_required`) | The node's agent predates agent-path shells, or the node is not Linux | Update the agent from **Packages → Updates → Agents** |
| **Open shell** fails with ``command `container_exec` is not allowed by this agent's local policy`` | The node was enrolled before container shells; updates do not turn them on | Add `"container_exec"` to `allowed_commands` in the agent's `agent.toml` and restart the agent. See [Configuration](/docs/reference/configuration/#agent-agenttoml) |
| **Open shell** fails with `the agent did not answer: update the agent` | The agent did not take the request within 15 seconds | Update the agent; check that the node is online |
| **Open shell** fails with `container shell is not supported on Windows nodes` | Windows node | Not supported |
| **Open shell** fails with `this node already has 8 open container shells` | The limit per node (or per user) is reached | Close a shell under **Remote sessions** |
| The shell closes at once | The container is not running, or its image has neither `bash` nor `sh` | Start the container; for images without a shell, use **Logs** |
| **Reveal values** is refused | `container.inspect_env` needs an MFA-verified session, or the node's agent is too old (`agent_update_required`) | Verify with MFA; update the agent from **Nodes → Updates** |

## Next steps

- [Docker images, volumes and networks](/docs/guides/docker-resources/)
- [Create a container](/docs/guides/create-container/)
- [Deploy a compose stack](/docs/guides/compose-stacks/)
- [App Store](/docs/guides/app-store/)
