---
title: Create a container
description: Run a single container on a node from a form, with ports, variables, volumes and limits, under the same policy as compose stacks.
sidebar:
  order: 7
---

This guide shows you how to run one container from an image without writing a
compose file. Forge saves it as a one-service
[compose stack](/docs/concepts/compose-stacks/), so you can update, roll back,
expose and take it down the same way.

## Before you begin

- The node runs Linux with **Docker Engine** from the App Store.
- You need `stack.manage` on the environment and `server.manage` on the node
  (**Operator**, **Admin** or **Platform admin**).
- Settings that give the container host access, such as **Network** `host`,
  need `stack.privileged` (**Admin** or **Platform admin**) with an
  MFA-verified session.
- Host paths for volumes must be inside an
  [approved file root](/docs/guides/approve-file-roots/) on the node.

## Steps

1. Open the node, select the **Docker** tab (**Containers**), then **Create** → **Create
   container**. You can also start from the stack list at `/stacks` →
   **Create container**.
   The **New container** page opens, with the node (and, from the stack
   list, the environment) already chosen. **Stacks** at the top takes you to
   the stack list.
   The form has the sections **Basics**, **Networking**, **Configuration**,
   **Resources & health** and a collapsed **Advanced**. On a wide window the
   **Summary** column on the right shows the resulting stack name, folder,
   image and ports.
2. Under **Basics**, choose the **Node** and the **Environment**, then enter
   the **Name** (it is also the stack name) and the **Image**, such as
   `nginx:1.27`.
3. Under **Networking**, select **Add port** for each container port. Leave
   the host port blank to use the same number. Choose **internal** (the
   node's internal address) or **public** (every address). Optional: choose
   **Expose via Nginx** (`http` or `https`), which marks the first TCP port
   for a site that you create later from the stack page, and a **Network**.
4. Under **Configuration**, select **Add environment variable** for each
   variable. Forge stores their values sealed in the stack's `.env` and never
   shows them again. Select **Add volume** and enter a volume name or a host
   path and the path in the container; tick **read-only** if needed.
   Optional: set **Command (optional)** and **Entrypoint (optional)**.
5. Optional: under **Resources & health**, choose **Restart**, **CPUs**,
   **Memory (MiB)** and a **Health check (optional)** with its **Interval**.
6. Optional: open **Advanced** to add **Labels** or set a **Folder
   (optional)**.
7. In the bar at the bottom of the page, leave **Start now** ticked and
   select **Create and start**. While a required field is missing, the bar
   and the **Summary** list what is still needed.

Forge shows the compose file it generated and follows the job. Select **Open
stack** to manage it.

## Verify

- The container appears in the node's **Docker → Containers** tab under **stacks**,
  and in the stack list (`/stacks`) as a stack of kind `container`, with state
  `running`.
- **Operations → Audit log** shows `stack.create` and `stack.deploy`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `enter an image such as nginx:1.27` | Empty image or spaces in it | Enter one image reference |
| `a stack named … exists` | The name is taken in the environment | Choose another name |
| `… is outside the stack folder and the approved file roots` | A host path outside approved roots | Approve the folder, or use a volume name |
| `needs stack.privileged` after choosing **Network** `host` | The host network gives the container the node's network | Ask an administrator to start it with MFA, or choose another network |
| The port is unreachable from your desk | Ports are internal by default | Mark the port **public**, or expose it through Nginx |

## Next steps

- [Deploy a compose stack](/docs/guides/compose-stacks/): update, roll back,
  expose and take down.
- [Containers](/docs/guides/containers/)
- [Approve file roots](/docs/guides/approve-file-roots/)
