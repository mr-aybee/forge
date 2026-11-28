---
title: Add a container registry
description: Store credentials for a private registry once, so nodes and Docker Swarm can pull and push its images.
sidebar:
  order: 8
---

This guide shows you how to add a private container registry to Forge. Forge
then uses its credentials whenever a node pulls or pushes one of its images,
a container service is deployed from it, or a Swarm service is deployed
from it.

## Before you begin

- You need `registry.manage` on the workspace: the **Workspace admin** or
  **Platform admin** role. It is MFA-guarded.
- Have a token with the least rights you need, for example a GitHub token
  with `read:packages` only, or `write:packages` to push.

## Steps

1. Go to **Settings** → **Registries** and select **Add registry**.
2. Choose the **Kind**: Docker Hub, GitHub (ghcr.io), GitLab, Amazon ECR or
   Custom. Forge fills in the host for the known kinds.
3. Enter the **Host**, for example `ghcr.io` or `registry.example.com:5000`.
   Docker Hub is always `docker.io`.
4. Enter the **Username** and the **Password or token**, then select
   **Add registry**. When you edit a registry, leave **Password or token**
   empty to keep the stored one.

Forge picks the registry by the image's host: `ghcr.io/acme/app:1` uses the
`ghcr.io` registry, and `nginx:1.27` uses Docker Hub.

:::caution
The secret is stored encrypted and never shown again. To change it, enter a
new one. Pushes are allowed only to registries added here.
:::

:::caution[Who uses the credentials]
- Anyone who may deploy a service or manage images on a node can pull with
  the workspace's registry credentials. Give the registry a read-only token
  unless you push from Forge.
- Pushes need **Allow push** on the registry and `registry.manage`
  (an administrator with MFA).
- Docker Swarm keeps the credentials of a deployed service on every manager
  (`--with-registry-auth`). After you change a token, deploy the service
  again so the managers drop the old one.
- Changing the host, kind or user name of a registry needs the secret again.
:::

## Verify

- On a node, open **Docker** → **Images** → **Pull image** and pull a private
  image from the registry. The job succeeds.
- Deploy a container service with a private image, for example
  `ghcr.io/<org>/<repo>/api:<tag>`, placed on one or more nodes. Each node's
  deploy job succeeds and the instance shows **State** `running`.
- Deploy a container service that uses a private image in a Swarm cluster.
  Its tasks start on every node.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `pull access denied` or `unauthorized` | Wrong user name or token, or the token lacks read rights | Update the registry with a new token |
| A service deploy fails with "the registry `<host>` refused to pull … without credentials" | No registry in Forge for the image's host, or its host is spelled differently (`ghcr.io` versus `https://ghcr.io/acme`) | Add a registry whose **Host** is exactly the image's host, then deploy again |
| A service deploy fails with "the registry `<host>` refused the credentials Forge has for it" | The registry's token expired or cannot read this image (for GitHub, `read:packages` and access to the package) | Update the registry with a token that can read the image, then deploy again |
| A service deploy fails with `unauthorized` although pulling the same image on the node's **Docker** page works | The node's agent is older than the server and pulls without credentials | Update the agent on the node, then deploy again |
| `push only to a registry added in Forge` | The image's host has no registry in Forge | Add the registry, or tag the image for one that is |
| ECR pulls fail after a few hours | ECR tokens expire after 12 hours | Update the secret, or use a long-lived credential |

## Next steps

- [Docker images, volumes and networks](/docs/guides/docker-resources/)
- [Run a Docker Swarm cluster](/docs/guides/docker-swarm/)
