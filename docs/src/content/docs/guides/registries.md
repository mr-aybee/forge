---
title: Add a container registry
description: Store credentials for a private registry once, so nodes and Docker Swarm can pull and push its images.
sidebar:
  order: 8
---

This guide shows you how to add a private container registry to Forge. Forge
then uses its credentials whenever a node pulls or pushes one of its images,
or a Swarm service is deployed from it.

## Before you begin

- You need `registry.manage` on the workspace: the **Workspace admin** or
  **Platform admin** role. It is MFA-guarded.
- Have a token with the least rights you need, for example a GitHub token
  with `read:packages` only, or `write:packages` to push.

## Steps

1. Go to **Settings** → **Registries** and select **Add registry**.
2. Choose the **Kind**: Docker Hub, GitHub (GHCR), GitLab, Amazon ECR or
   Custom.
3. Enter the **Host**, for example `ghcr.io` or `registry.example.com:5000`.
   Docker Hub is always `docker.io`.
4. Enter the **User name** and the **Password or token**, then select
   **Save**.

Forge picks the registry by the image's host: `ghcr.io/acme/app:1` uses the
`ghcr.io` registry, and `nginx:1.27` uses Docker Hub.

:::caution
The secret is stored encrypted and never shown again. To change it, enter a
new one. Pushes are allowed only to registries added here.
:::

## Verify

- On a node, open **Docker** → **Images** → **Pull image** and pull a private
  image from the registry. The job succeeds.
- Deploy a container service that uses a private image in a Swarm cluster.
  Its tasks start on every node.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `pull access denied` or `unauthorized` | Wrong user name or token, or the token lacks read rights | Update the registry with a new token |
| `push only to a registry added in Forge` | The image's host has no registry in Forge | Add the registry, or tag the image for one that is |
| ECR pulls fail after a few hours | ECR tokens expire after 12 hours | Update the secret, or use a long-lived credential |

## Next steps

- [Docker images, volumes and networks](/docs/guides/docker-resources/)
- [Run a Docker Swarm cluster](/docs/guides/docker-swarm/)
