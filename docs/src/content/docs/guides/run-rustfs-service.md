---
title: Run RustFS as a service
description: Run a RustFS S3-compatible object store as a Forge container service from a services file.
sidebar:
  order: 9
---

You can run RustFS as an ordinary Forge container service, described in a
services file. Forge places, deploys and restarts it like any other
service. It does not manage buckets or access keys: you do that in the
RustFS console or with any S3 tool.

## Before you begin

- `deploy` on the environment and `server.manage` on the node (roles
  `operator` and up).
- A Linux node with Docker Engine in the environment's cluster. The first
  deploy installs Docker Engine when it is missing, which needs
  `package.manage` with an MFA-verified session.
- Ports `9000` (S3 API) and `9001` (console) free on the node.
- The `forge` CLI signed in.

:::note
RustFS reads its root credentials from the `RUSTFS_ACCESS_KEY` and
`RUSTFS_SECRET_KEY` environment variables. It has no documented variant
that reads them from a file, so this guide stores them as Forge secret
variables, sealed on the server, rather than as a mounted secret file.
:::

## Steps

1. Write `rustfs.yaml`. Replace `<tag>` with a RustFS release tag and
   `<node-name>` with the node:

   ```yaml
   name: storage
   services:
     rustfs:
       image: rustfs/rustfs:<tag>
       ports: ["9000:9000", "9001:9001"]
       volumes: ["rustfs-data:/data"]
       environment:
         RUSTFS_VOLUMES: /data
         RUSTFS_ADDRESS: 0.0.0.0:9000
         RUSTFS_CONSOLE_ENABLE: "true"
         RUSTFS_CONSOLE_ADDRESS: 0.0.0.0:9001
       restart: unless-stopped
       deploy:
         placement:
           constraints: ["node.hostname == <node-name>"]
       x-forge:
         console: { port: 9001, path: "/", scheme: http }
   volumes:
     rustfs-data: {}
   ```

   The placement constraint keeps the data volume on one node. The
   `console` link adds **Open console** to the service page.

2. Show what would change:

   ```sh
   forge services plan -f rustfs.yaml --environment <environment>
   ```

3. Apply the file:

   ```sh
   forge services apply -f rustfs.yaml --environment <environment> --yes --wait
   ```

   :::caution
   Until the next step, RustFS runs with its default root credentials
   (`rustfsadmin`). Do not expose ports `9000` and `9001` beyond a trusted
   network before you set your own.
   :::

4. Put the root credentials in two files, readable only by you, and store
   them as secret variables. The CLI reads the values from the files and
   never from the command line:

   ```sh
   forge services update rustfs --environment <environment> \
     --secret-env RUSTFS_ACCESS_KEY=@rustfs-access-key.txt \
     --secret-env RUSTFS_SECRET_KEY=@rustfs-secret-key.txt --yes
   ```

   Later applies of `rustfs.yaml` keep these secret variables.

5. Deploy so RustFS restarts with the new credentials:

   ```sh
   forge services deploy rustfs --environment <environment> --wait --yes
   ```

6. Delete the two credential files, or keep the values in the
   [credential vault](/docs/guides/credential-vault/) instead.

## Verify

```sh
# The instance is running on the node
forge services show rustfs --environment <environment>
```

Open the service page and select **Open console**. Sign in with your access
key and secret key. Point S3 clients at `http://<node-address>:9000`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Apply exits with `7` | Another services file or a stack manages a service named `rustfs` | Rename the service in the file |
| The container restarts in a loop | The data volume is not writable, or a port is taken | Read the logs with `forge services logs rustfs --node <node-name> --environment <environment>` |
| Sign-in fails with your keys | The deploy in step 5 did not run | Deploy again and check that the job succeeded |
| **Open console** is missing | No `x-forge.console` in the file | Add it and apply again |

## Next steps

- A later release adds RustFS back to the [App Store](/docs/guides/app-store/)
  as a package with a container template.
- [Services file reference](/docs/reference/services-yaml/)
- [Adopt a container as a service](/docs/guides/adopt-container/) for a
  RustFS container you already run.
