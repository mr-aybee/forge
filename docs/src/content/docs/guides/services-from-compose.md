---
title: Run services from a Compose file
description: Describe services in a Compose / Swarm format file and let Forge plan, place and deploy them on your nodes.
sidebar:
  order: 7
---

You can keep your services in a Compose / Swarm format file and apply it
with Forge. Each entry under `services:` becomes a Forge service, placed on
the right nodes and deployed without Docker Swarm.

## Before you begin

- The `deploy` action on the environment (roles `developer` and up) and
  `server.manage` on the nodes the services go to. Privileged settings
  (`cap_add`, `devices`, `sysctls`, host paths outside approved roots) need
  `stack.privileged` and an MFA-verified session.
- An environment in a cluster with Linux nodes that run Docker Engine. Docker
  Swarm clusters are not supported; use a swarm stack there.
- The `forge` CLI signed in, or the console.
- The keys Forge reads are in the [services file reference](/docs/reference/services-yaml/).

## Steps

1. Write the file, `compose.yaml`, with a top-level `name`:

   ```yaml
   name: shop
   services:
     web:
       image: nginx:1.27
       ports: ["8080:80"]
       deploy:
         replicas: 2
   ```

2. Show what would change:

   ```sh
   forge services plan -f compose.yaml --environment <environment>
   ```

3. Apply it and wait for the deploys:

   ```sh
   forge services apply -f compose.yaml --environment <environment> --yes --wait
   ```

4. Change the node count later without editing the file:

   ```sh
   forge services scale web 3 --environment <environment> --yes
   ```

5. Write the current state back to a file:

   ```sh
   forge services export --environment <environment> > compose.yaml
   ```

In the console, open **New service** or a service and use the **YAML** tab:
edit the file, choose **Plan** to see the changes and **Apply** to run them.

### Mount secrets and configs

Store each value as a credential vault item first (**Vault** → **New item**;
the password field holds the value). Then declare it `external: true` at the
top level and list it under the service:

```yaml
services:
  api:
    image: registry.example/api:2
    secrets: [db_password]
secrets:
  db_password: { external: true, name: "DB password" }
```

The container reads it at `/run/secrets/db_password`. Applying a file that
adds a vault item needs a platform administrator with MFA. When you rotate
the item, the service shows **deploy it again** until its next deploy. See
[Secrets and configs](/docs/reference/services-yaml/#secrets-and-configs).

## Verify

- `forge services plan` on the same file shows every service as `unchanged`.
- `forge services list --environment <environment>` shows the services, with
  one running instance per node.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `services.web.build: Forge runs images …` | The key is not supported | Remove it; the message names each refused key |
| `… 3 nodes wanted, 2 eligible` | Too few nodes match: offline, drained, wrong cluster, constraints or reservations | Lower `replicas`, add nodes or relax the constraints |
| `… is managed by the services file …` (409) | Another file applied a service of that name | Change it in that file, or rename it |
| `… changed since the plan was made` (409) | Someone saved the service in between | Run the apply again |
| `secrets.db.file: the server never reads files …` | A secret or config uses `file:` | Store the value in the vault and declare it `external: true` |
| `no credential vault item is named …` | The name does not match a vault item | Fix `name:` or create the item |
| Deploy fails with `… update the agent first` | The node's agent cannot write secret files | Update the agent on that node |
| A removed service stays listed for a while | Its `remove` jobs are still running | It is deleted once its instances are gone |

## Next steps

- [Services](/docs/guides/services/) for single services in the console.
- [Services file reference](/docs/reference/services-yaml/).
- [`forge services` CLI reference](/docs/reference/cli/services/#services-files).
