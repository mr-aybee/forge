---
title: Services
description: Create systemd and container services in a project's environment, place them on nodes, deploy them and manage their environment variables.
sidebar:
  order: 6
---

This guide shows you how to create a service in a project's environment,
place it on nodes and deploy it, follow its logs and manage its environment
variables. For the model behind projects, environments and services, see
[Projects, environments and services](/docs/concepts/projects-environments-services/).

## Before you begin

| Action | Needs |
| --- | --- |
| Create a project or an environment | `workspace.manage` on the workspace, or on the project for its environments (**Admin** or **Platform admin**) |
| Create, edit, deploy, start, stop, remove a service | `deploy` on the environment (**Developer**, **Operator**, **Admin** or **Platform admin**) |
| Place on a node | also `server.manage` on that node (**Operator** or higher) |
| Install missing runtimes on deploy | `package.manage` on the node, with an MFA-verified session |
| Status and logs | `view` on the environment |

- Nodes run Linux. Container services need **Docker Engine**. The first
  deploy installs it from the [App Store](/docs/guides/app-store/) on a node
  that has none; see
  [`forge services deploy`](/docs/reference/cli/services/#forge-services-deploy).
- Code delivery is up to you: copy it with the
  [file explorer](/docs/guides/manage-files/) or the
  [browser terminal](/docs/guides/terminal/), or use a container image.
  Deploying from GitHub is on the [roadmap](/docs/roadmap/).

## Create the project and an environment

Services live in an environment, and environments belong to a project, so
create those first. You do this once per product and once per stage.

1. Open **Projects** in the sidebar and select **New project**. Enter the
   **Name** (the product, for example `shop`; lower-case letters, digits and
   dashes) and a **Description**. Select **Create**.
2. Select the project in the tree and select **New environment**. Enter the
   **Name** (free text: `test`, `staging`, `uat`, `production`…), pick the
   **Cluster** it runs in and tick **Protected** for production-like stages.
   Select **Create**.

The **Unassigned** bucket in the tree lists environments without a project.
Open an environment's row menu and select **Attach to project…** to place it.

## Create and deploy a service

1. In **Projects**, open the environment's row menu and select **New service
   here**. You can also open **Services** and select **New service**; then
   pick the **Project** and the **Environment** under **Basics**. The
   cluster follows from the environment.
2. At the top, pick the kind:

   | Kind | Runtime | What runs | On the node |
   | --- | --- | --- | --- |
   | **Systemd** | `systemd` | A long-running command (gunicorn, `dotnet app.dll`, `java -jar`, `php artisan queue:work`, `node server.js`…) | systemd unit `forge-app-<env>-<svc>.service` (`Restart=always`), or Forge's supervisor where there is no systemd |
   | **Container** | `container` | A Docker image with ports, volumes and an optional command | Container `forge-app-<env>-<svc>`, restart unless stopped; it has its own [container page](/docs/guides/containers/) |

3. Under **Basics**, enter the **Name** (lower-case letters, digits and
   dashes). Together with the environment it names the unit or container on
   the node: `forge-app-<environment>-<service>`. Under **Placement**, pick
   the nodes; each chosen node runs a replica.
4. Fill in the other sections (see
   [Configuration sections](#configuration-sections)). The rail on the left
   marks each section as complete, needing input or in error; select one to
   scroll to it.
5. Select **Create service**, or **Create and deploy** to deploy at once
   (needs at least one node).

   For a systemd service on a Linux node, Forge prepares each placed node in
   a `prepare` job: it creates the service's account and its folder
   `/srv/forge/apps/<environment>/<service>`, and approves a writable file
   root on that folder. Nothing runs yet. You can upload the code into the
   folder first, in the node's **Files** tab with the root named after the
   service (see [Manage files](/docs/guides/manage-files/)).
6. If you did not deploy in step 5, select **Deploy** on the service page.

Each node gets its own durable job. **Start**, **Stop**, **Restart** and
**Logs** are per node: **Logs** is in the node's row, the others are in its
**⋯** menu. To add nodes later, use **Add node** under the node list on the
service's **Overview** tab.

## Run on several nodes

A service becomes highly available when it runs on two or more nodes of the
environment's cluster with an ingress endpoint in front. Each node has a
role:

| Role | What it means | What Nginx does with it |
| --- | --- | --- |
| **Replica** (active) | Serves traffic alongside the other active nodes | A normal upstream, chosen by the endpoint's **Balancing** rule |
| **Standby** | Takes over only when every replica is down | An upstream marked `backup` |

1. Under **Add node**, select another node. In the dialog **Run `<service>`
   on `<node>`**, choose **Replica** for active-active or **Standby** for
   active-standby, then select **Confirm**. For a container service the
   node needs **Docker Engine**; the deploy installs it when the node has
   none.
2. Select **Deploy**. Each node gets its own deploy job and its own copy of
   the service, with the same configuration and environment variables.
3. In the **Routes** panel, select **Expose through ingress** (also in the
   **⋯** menu next to **Deploy**). The endpoint
   editor opens with this service as the target and its port filled in.
   Enter the **Domains**, pick **Balancing** (**Round robin**, **Least
   connections** or **IP hash**; shown when the service has two or more
   replicas), choose **TLS** and save.
4. Open **Ingress**, filter to the cluster and select **Apply to edges** on the
   endpoint (needs `proxy.manage` with MFA). See
   [Ingress](/docs/guides/ingress/).

Each node row shows its role as a `replica` or `standby` chip. Change it at
any time with **Make it a standby** or **Make it a replica** in the row's
**⋯** menu; apply the endpoint again so Nginx picks up the change. The **Routes** panel lists
every endpoint that targets the service, with its balancing rule and the
number of replicas and standbys it fans out to. On **Ingress → Routing**, filtered to the cluster, each upstream carries a `replica` or `standby` badge.

:::note
Two replicas of a stateful service, such as a database, do not replicate
their data; Forge only balances the connections. Use the engine's own
replication, or run one replica and one standby that share storage.
:::

:::caution
Forge does not yet take a replica out of the endpoint when its health check
fails. Nginx skips an upstream after `max_fails` connection failures and
retries it later; a replica that answers but is unhealthy keeps receiving
requests until you stop it or change its role.
:::

## Do it from the command line

The [`forge` CLI](/docs/reference/cli/) covers the whole path with typed
commands: project, environment, service, placement, deploy and ingress.
The example publishes a container service `web` of the project
`shop` on two nodes of the cluster `lab`, with `lab-b` as the ingress edge.
Replace the names with yours.

You need, in addition to the roles under
[Before you begin](#before-you-begin):

- `workspace.manage` on the workspace to create the project and the
  site and to save the cluster's ingress profile, and `proxy.manage` on the
  edge with an MFA-verified session to apply the profile and the site. In a
  script, run `forge auth step-up` first.
- **Nginx** on the edge `lab-b`, from the [App Store](/docs/guides/app-store/)
  ([`forge packages install`](/docs/reference/cli/packages/#forge-packages-install)).
- `package.manage` on `lab-b` and `lab-c` with an MFA-verified session when
  they have no **Docker Engine** yet: the first deploy installs it.

1. Create the project.

   ```sh
   forge projects create shop --description "Web shop"
   ```

2. Create its production environment in the cluster.

   ```sh
   forge environments create shop-prod --cluster lab --project shop
   ```

3. Create the service. Nothing runs yet.

   ```sh
   forge services create --environment shop-prod --name web --runtime container --image nginx:1.27 --port 8080:80
   ```

4. Place it on two nodes: one active, one standby.

   ```sh
   forge services place web --node lab-b:active --node lab-c:standby --yes
   ```

5. Deploy it and wait for the jobs.

   ```sh
   forge services deploy web --wait --yes
   ```

6. Save the cluster's ingress profile with `lab-b` as its edge.

   ```sh
   forge clusters ingress set lab --edge lab-b --yes
   ```

7. Push the profile to the edge.

   ```sh
   forge clusters ingress apply lab --yes
   ```

8. Write the site spec. `targets` points at the service, so Forge fills in
   the upstreams from the nodes it runs on; leave `upstreams` out.

   ```sh
   CLUSTER_ID=$(forge -o json clusters get lab | jq -r .result.id)
   SERVICE_ID=$(forge -o json services show web --environment shop-prod | jq -r .result.id)
   cat > site.json <<EOF
   {
     "name": "shop",
     "domains": ["shop.example.com"],
     "cluster_id": "$CLUSTER_ID",
     "targets": [{ "kind": "app", "service_id": "$SERVICE_ID", "port": 8080 }]
   }
   EOF
   ```

9. Create the site.

   ```sh
   forge sites create --file site.json
   ```

10. Push the site to the edge.

    ```sh
    forge sites apply shop --yes
    ```

Check the result:

```sh
# RUNNING shows the instances the nodes report as running
forge services list --environment shop-prod
# The edge reports `applied`
forge clusters ingress get lab
# The site, its edge and the upstream addresses behind it
forge clusters routing lab
```

| Symptom | Cause | Fix |
| --- | --- | --- |
| Step 5 exits with `4` or `5` | A node has no Docker Engine, and installing it needs `package.manage` with an MFA-verified session | Run `forge auth step-up`, or ask an **Operator**, then deploy again |
| Step 5 fails with `Docker Engine failed to install on this node` | The install job failed (no route to `download.docker.com`, an unsupported OS release) | Read the `package.action` job with [`forge jobs get`](/docs/reference/cli/jobs/#forge-jobs-get), fix the cause, then deploy again |
| Step 5 fails with `Docker is not available on this node` | The node was offline or did not answer when the deploy started, so nothing was installed | Bring the node online, then deploy again |
| Step 7 or 10 exits with `5` | `proxy.manage` needs an MFA-verified session | Run `forge auth step-up`, then repeat the step |
| Step 7 exits with `10` (`ingress_failed`) | Nginx is missing on the edge, or `nginx -t` refused the configuration | Read the `ERROR` column; install **Nginx** on the edge, then apply again |
| Step 9 exits with `2` and "this cluster has no ingress profile yet" | Step 6 was skipped | Save the profile, then create the site |

Every command and flag is in the CLI reference:
[`forge projects`](/docs/reference/cli/projects/),
[`forge environments`](/docs/reference/cli/tenants-environments/),
[`forge services`](/docs/reference/cli/services/),
[`forge clusters ingress`](/docs/reference/cli/clusters/#forge-clusters-ingress-set)
and [`forge sites`](/docs/reference/cli/sites-certificates/).

## Follow live logs

In the **Overview** tab, select **Logs** in a node's row. The logs open as a
tab in the bottom panel, next to any terminals, titled `<service> (<node>)`,
for example `api (lab-b)`. Following starts at once and runs until you press
**Pause following** or close the tab; selecting **Logs** again for the same
service and node switches to the open tab. Tabs stay open while you move
around the console, so you can follow several services and nodes and switch
between them. **Logs** is disabled until the service is deployed to the node.
The node's **Services** tab has the same **Logs** action.

New lines appear within about a second. Following
keeps going while the service is stopped, started or restarted, so the
startup output of the new process is captured too. Forge reads by position,
not through an open stream:

- the systemd journal cursor for systemd services;
- the log file offset where there is no systemd;
- Docker timestamps for containers. A container that is removed and created
  again is picked up when it comes back.

You can filter lines, clear the view, or download what you have. The view
stays on the newest line until you scroll up. The output of the last deploy
job on a node is under **Deployment output** in the row menu. The last 5,000 lines are kept in the browser. Each
segment runs about 50 seconds on the node and is a normal `app.action` job
(`follow`, needs `view` on the environment).

## Set environment variables

Edit them in the grid or **Paste .env** (`KEY=value`, `export`, quotes and
comments are understood). Every variable is delivered in all three ways the
runtimes expect:

1. Command-line arguments are part of the command itself.
2. Environment file `/etc/forge/apps/<environment>/<service>.env` (mode 0600, root):
   the service's `EnvironmentFile=` — `KEY=value` lines.
3. Containers receive the same variables as their environment (and the
   same file is written for reference / `--env-file`).

**Package variables:** tick installed App Store packages to load their
`/etc/forge/pkgs/<package>.env` first, e.g. `FORGE_PKG_POSTGRESQL_URL` or
`FORGE_PKG_REDIS_URL` — generated passwords never have to be copied anywhere.

**Secrets:** mark a variable as secret (names like `*_PASSWORD`, `*_TOKEN`,
`*_KEY` are marked automatically). Its value is sealed with the vault envelope,
never returned by the API or shown again, and only unsealed into the signed
command that deploys the service. Leave a secret empty when saving to keep it.

Values may not contain line breaks, so one file works for systemd and Docker
without quoting rules.

**Log URL:** Forge sets `FORGE_LOG_URL` to the node's local log listener:
`http://127.0.0.1:5341` for systemd services and `http://forge-node:5341`
for containers, which get `--add-host forge-node:host-gateway`. A
`FORGE_LOG_URL` you set yourself wins. See
[Send and search service logs](/docs/guides/application-logs/).

## Mount secrets and config files

A service can mount credential vault items as read-only files. Only the
item's name is stored with the service; the value stays sealed in the vault
until a deploy writes it on the node.

1. Create the vault item (**Vault** → **New item**); its password field is the value (up to 4096 bytes).
2. Open the service, go to **Secrets & configs** and choose **Add file**.
3. Pick **Secret** or **Config**, choose the vault item by name, and set the target path and, if needed, the mode and owner uid.
4. Save and **Deploy**.

On the node the agent writes each file under
`/var/lib/forge-agent/secrets/<environment>/<service>/` (a folder only root
can write), atomically, and removes files the service no longer lists.

| Runtime | Where the service sees the files |
| --- | --- |
| Container | Each file bind-mounted read-only at its target. Secrets default to `/run/secrets/<name>` with mode `0400`, configs to mode `0444`, owner root unless you set a uid |
| Systemd | In the folder named by the `FORGE_SECRETS_DIR` variable, owned by the service's account; each file is named after its target's last part |

From the command line:

```sh
forge services update api --environment shop-prod \
  --secret "DB password:db_password" --config "api-config:/etc/api/app.conf"
```

:::note
Referencing a vault item needs a platform administrator with MFA
(`secret.read`): a signed-in session, or an elevated API key (from CI, for
example). A plain API key is refused with `403`; sign in with
`forge auth login` or create an elevated key (`forge api-keys create --elevated`). Anyone who may deploy the service can
deploy it again afterwards. Rotating or renaming the item marks every service
that mounts it: the service page and `forge services show` say it needs a
deploy, until the next successful one. Swarm services use Swarm secrets
instead. Nodes need an agent of this release or later; older agents fail the
deploy with "update the agent first".
:::

## Layout and isolation

| Path | Contents |
| --- | --- |
| `/srv/forge/apps/<environment>/<service>/` | Code and data, owned by the app's user (0750) |
| `/srv/forge/apps/<environment>/<service>/.forge/run.sh` | Launcher (`exec <command>`), root-owned |
| `/etc/forge/apps/<environment>/<service>.env` | Environment: native apps `root:<account>` 0640, containers 0600. systemd reads it as root and passes the variables to the app's processes; no other account can read it |
| `/var/log/forge/apps/<environment>-<service>.log` | Output when supervised without systemd (journald otherwise) |

Native apps never run as root. Each runs as its own system account, named
after the app so you can tell it apart in `ps`, `ls -l` and the audit log:

| | |
| --- | --- |
| Name | `<environment>-<service>`, for example `staging-api`. When that is longer than 32 characters, or an account Forge did not create already has it, Forge shortens it and adds a 6-character hash, for example `staging-api-3f9a1c` |
| Comment (GECOS) | `Orbyn Forge app <environment>/<service>`. Forge only ever uses an account that carries exactly this comment, so it never takes over a system or human account |
| Home | `/srv/forge/apps/<environment>/<service>` (0750, owned by the account) |
| Shell | `nologin` (`/usr/sbin/nologin` or `/sbin/nologin`, depending on the distribution) |
| Group | Its own group of the same name |

The account stays when you remove the app, with the app's files. The app's
automatic file root on that node is removed (see
[App roots](/docs/concepts/file-explorer/#app-roots)). Units use
`NoNewPrivileges`, `PrivateTmp` and `ProtectSystem=full`.

:::note[Apps deployed by an older Forge]
Older releases named the account `fa-<app>-<hash>`. The next deploy renames
it to the new name and keeps its user ID, so the app keeps owning its
files. Forge stops the app for the rename and starts it again as part of the
deploy.
:::

:::caution
The app's user owns its folder, so it could put a link where `.forge` should
be. The helper never follows one: it refuses to deploy while `.forge` is a
symlink, not a folder, or not owned by root, and the deploy fails with
`… is a symlink or not a directory` or `… is not owned by root`. Remove
`<app>/.forge` as root and deploy again; Forge recreates it.
:::

## The service page

The page has the tabs **Overview**, **Configuration**, **Health**, **Storage** and **Releases**. The first two are:
- **Overview:** the service at a glance.
  - A status strip: **Replicas** (how many run and are healthy, for example `2/2 running · healthy`), **Last deploy** (result, when, who, and the release number when the service uses releases), **Endpoint** (the first route's domain) and **Log ingest keys** (click to jump to them).
  - **Nodes**: one line per node with its role chip, the last deploy result (`deployed`, `failed`…), the live state (`running`, `stopped`…) and health. **Logs** follows the output; the **⋯** menu (or a right-click on the row) has **Deployment output**, **Start**, **Stop**, **Restart**, **Log tail (one-off)**, the role change and **Remove from node**. **Add node** is under the list. The same list, per node, is on the node page's **Services** tab.
  - **Summary**: the type, the full command or image (with a copy button), runtimes, limits and variables, and under **On the node** where the service lives: **Service directory**, **Environment file**, the systemd unit or container, the user it runs as and its file roots.
  - **Routes**, **Log ingest keys**, the service's ports, and the log of the last action you ran.

  On wide windows **Nodes** and **Summary** sit side by side; on narrow ones they stack.
- **Configuration:** the service editor, with the same sections as on create. While you have unsaved changes a bar at the bottom offers **Discard**, **Save** and **Save and deploy**.

**Deploy** is the main action in the page header. Next to it,
**Open console** and **Open shell** appear when they apply; see
[Open a service's console or shell](#open-a-services-console-or-shell).
The **⋯** menu next to it
holds **Expose through ingress**, **Open Ingress** and **Delete service**.
**Delete service** asks for confirmation and is disabled until you remove the
service from every node (node row **⋯** → **Remove from node**).

Tags under the title show the app's **service IP** and its internal names
(`api.payments`, `api.payments.lab.forge.internal`). Click a tag to copy it.
Node and cluster pages have the same tags for the node's mesh address and
block, and the cluster's subnet and domain.

### Open a service's console or shell

Many services serve an admin page, such as a database UI or an object
store console. Give the service a web console link and the page header shows
**Open console**.

1. Open the service and select **Configuration**.
2. In **Network** (a container) or **Start command** (a native service), set
   **Web console port**: the port the admin page listens on. For a container
   it is the published host port, so publish it under **Published ports**
   too.
3. Optional: set **Console path** (default `/`) and **Console scheme**
   (`http` or `https`, default `http`).
4. Select **Save**. Leaving **Web console port** empty removes the link.

**Open console** opens `scheme://<address>:<port><path>` in a new tab. The
address is the host name of an ingress endpoint that routes to the service
at the console port, with that endpoint's own scheme and port. Without one,
it is the address of the node the service runs on; with several nodes the
button opens a menu with one entry per node. Forge does not proxy the page
or sign you in: your browser must reach that address, and the page asks for
its own credentials.

**Open shell** opens a terminal in the panel at the bottom of the window:

| Service | What opens | Needs |
| --- | --- | --- |
| Container | A shell inside its container on the node (`docker exec`), audited as `container.exec` | `container.exec` on the node |
| Native | The node's SSH terminal | `terminal.open` on the node |

The button lists only the nodes you may open a shell on, and is hidden when
there are none. A service in a Docker Swarm cluster has no **Open shell**:
open its task container from the node's **Containers** tab.

The same link is set with `forge services create|update --console-port`
(see the [CLI reference](/docs/reference/cli/services/#forge-services-update))
or with `x-forge.console` in a
[services file](/docs/reference/services-yaml/#x-forgeconsole).

### State

| **State** | Meaning |
| --- | --- |
| `running` | The node last reported the service or container as running |
| `stopped` | The node reported it stopped (inactive or exited) |
| `failed` | The service failed, or its last deployment failed. Hover for the error |
| `node offline` | The node's agent is offline, so Forge cannot tell. Hover for the last reported state |
| `deploying`, `starting`, … | A job is running on that node |
| `drifted` | The container on the node runs another release than the last deploy. Deploy again to clear it |
| `stranded` | The node is offline past the grace period or drained. Forge moved the service, or it has local data and stays (alert `service-stranded`) |
| `not deployed` | Placed on the node but not deployed yet |
| `unknown` | The node has not reported a state yet |

**State** combines the node's agent status with the state the node last
reported for the service. It is not a health check: the **Health** column is.
It shows **Healthy**, **Degraded**, **Unhealthy** or **Unknown** per node;
select it to see each check and why it isn't passing. An amber banner appears
when two services on one node declare the same **Listening port**. It updates live when a
node goes offline or comes back. For startup, readiness and liveness checks,
including Forge's default check that connects to the service's **Listening
port**, see [Configure health checks](/docs/guides/health-checks/).

## Keep services running

Forge's service reconciler checks every instance every 15 seconds and acts
as the person who ran its last job (deploy, start or restart). It skips a
service while a deployment of it runs. Each action is a job under
**Operations → Jobs** and is audited as `system:reconciler`.

- **Exited container.** On an online node, an instance that should run
  (you did not stop it) and whose container exited is started again; a
  missing container is deployed again. Forge waits 2 minutes between tries
  and gives up after 5 tries in 30 minutes: the instance turns `failed` with
  the reason. Start or deploy it by hand after you fix the cause. Systemd
  services are started again when their last reported state is `failed`.
- **Drift.** A container that runs another release than the last deploy
  marks the instance `drifted`. Forge does not redeploy on its own.
- **Lost node.** When a node is offline longer than
  `services.node_grace_secs` (default 300), or drained, its instances turn
  `stranded`. A container with no volumes, devices or env files is first
  deployed on another online, undrained Linux node of the same cluster that
  does not run it yet, the one with the fewest services first. When the old
  node answers again, Forge removes the old instance. Anything else stays
  and raises the `service-stranded` alert; it counts as deployed again when
  its node is back and not drained. Set `on_node_failure` to `move` or
  `alert` in the service spec to choose (see
  [service spec fields](/docs/reference/cli/services/)).
- **Drain.** On a node's page select **Drain** (or run
  `forge nodes drain <node-name>`; needs `node.drain`, **Operator** or
  higher). New placements skip the node and movable services leave it.
  **Undrain** takes it back.

:::note
If the person who last ran an instance was removed or disabled, Forge does
nothing for it and audits `service.reconcile.refused`. Deploy it again as
someone else.
:::

:::caution
Agents older than this release do not report container states, so exited
containers on those nodes are not restarted. Update the agent.
:::

## Runtime dependencies

Systemd services declare the runtimes they need under **Runtimes** with
**Add a runtime**: PHP-FPM (choose the version), Python, Node.js,
Java (Eclipse Temurin), .NET (ASP.NET Core runtime, runtime only, or SDK) and
Go. Add none for a self-contained app, such as a Go binary or a
self-contained .NET publish.

On **Deploy**, Forge installs any runtime that is missing on a node. These are
normal App Store installs: they need `package.manage` and an MFA session, they
appear under **Packages → Installed**, and their output streams live. The deploy waits for
them (up to 45 minutes) and fails clearly if an install fails. Installs on one node run one at a time, even when one deploy needs
several runtimes: the node's package manager allows only one run at a time.
On Debian and Ubuntu, each `apt-get` run also waits up to five minutes for
the dpkg lock when something else holds it (for example
`unattended-upgrades`); `dnf` on RHEL-likes waits by itself. An install may
run for up to 45 minutes on the node, and the deploy waits for it, so a lock
wait never times out an install. The app's launcher
puts each runtime on `PATH` and sets its usual variables:

| Runtime | What the launcher sets |
| --- | --- |
| .NET | `DOTNET_ROOT` |
| Go | `GOROOT` |
| Java | `JAVA_HOME` |
| PHP | the chosen version answers to `php` (through `<app>/.forge/bin`) |

Each runtime's `/etc/forge/pkgs/<id>.env` (`FORGE_PKG_*` variables) is loaded too.

## Configuration sections

The editor shows the sections of the kind you picked. On edit, changing the
kind asks first and clears the other kind's fields.

| Systemd section | Contents |
| --- | --- |
| **Basics** | Project, environment, name, placement |
| **Runtime** | The runtimes the service needs (see [Runtime dependencies](#runtime-dependencies)) |
| **Start command** | Command, working directory, **Listening port**, **Restart policy** (and **Max restart attempts** for **On failure**) |
| **Limits** | CPU and memory limits (systemd `CPUQuota=`, `MemoryMax=`) |
| **Environment variables** | **Grid** or **Bulk edit**, **Paste .env**, package variables |
| **Storage** | Service directory, environment file |
| **Health checks** | The default TCP check, with a link to the **Health** tab |
| **Advanced** | Unit name, runs-as user |

| Container section | Contents |
| --- | --- |
| **Basics** | Project, environment, name, placement |
| **Image** | The image, command and entrypoint |
| **Runtime** | **Restart policy** (**Unless stopped**, **Always**, **On failure** with **Max restart attempts**, **Never**), stop timeout, user, host name, **Run an init process** |
| **Network** | Published ports, **Cluster network**, extra Docker networks, extra `/etc/hosts` entries, DNS servers |
| **Storage** | Volumes and tmpfs mounts (target and size in MiB) |
| **Limits** | CPU and memory limits, ulimits (soft / hard, `-1` = unlimited) |
| **Security** | Read-only root filesystem, dropped and added capabilities, devices (`/dev/…`, permissions from `rwm`), sysctls |
| **Environment variables** | **Grid** or **Bulk edit**, **Paste .env**, package variables |
| **Health checks** | Docker `HEALTHCHECK`: command, interval, timeout, retries, start period |
| **Advanced** | Log rotation size, labels; **Form / YAML** toggle |

**Privileged settings.** Added capabilities, devices and sysctls, like host
paths outside the app's folder, carry a **needs stack.privileged** badge.
You can save them; deploying needs `stack.privileged` (**Admin** or
**Platform admin**) with an MFA-verified session.

**Concurrent edits.** The editor saves against the version it loaded. If
someone else saved the service in the meantime, **Save** stops with a notice
that keeps your edits on screen; **Reload** loads the latest version and
discards them. Validation errors from the server show above the editor.

**Volumes.** A container volume is `<host path>:<container path>[:ro]`.
Which host paths you can mount depends on your rights:

| Host path | Who can deploy it |
| --- | --- |
| Inside the app's folder `/srv/forge/apps/<environment>/<service>/`, or inside an [approved file root](/docs/guides/approve-file-roots/) of the node | Anyone with `deploy` on the environment (**Developer** and up) |
| Anywhere else, such as `/home`, `/opt/<vendor>` or another app's folder | `stack.privileged` (**Admin** or **Platform admin**) with an MFA-verified session |
| The Docker socket, system paths (`/`, `/etc`, `/root`, `/usr`, `/var/lib/docker`, …) and Forge's folders (`/opt/forge`, `/etc/forge`, `/var/lib/forge-agent`) | No one. Saving the app returns an error |

:::caution
A container usually runs as root. A writable mount of a host folder lets it
change everything in that folder as root, so a folder such as
`/var/spool/cron` or `/home` gives root on the node or another user's
account. That is why such mounts need an administrator with MFA.
:::

Forge checks at each deploy, for each node, against that node's approved
roots. A deploy that needs `stack.privileged` re-checks it at every job step
and refuses a mount added to the app after the deploy started. The audit
event `service.deploy` lists the mounts in `privileged_mounts`.

**Private images.** A container image on a private registry, for example
`ghcr.io/<org>/<repo>/api:<tag>`, needs that registry in Forge first (see
[Add a container registry](/docs/guides/registries/)). Each deploy then
pulls the image with the registry's credentials on every node, including
nodes that pull a digest pinned by an earlier node. The credentials are
looked up by the image's host when the deploy runs and go to the node only
inside the signed command, never into the service, the job or the audit
log. Anyone with `deploy` on the environment can deploy a private image
this way.

**Environment.** The grid has a name, a value and a secret flag on each row.
**Bulk edit** switches to `KEY=value` text. Stored secrets show as
`<sealed>`; keep that to leave a secret unchanged.

**YAML.** Under **Advanced**, containers can switch from **Form** to **YAML** and edit the whole
container as a docker-compose service:
`image`, `command`, `entrypoint`, `user`, `hostname`, `restart` (`on-failure:N` included), `ports`,
`volumes`, `environment`, `extra_hosts`, `labels`, `read_only`,
`stop_grace_period`, `healthcheck`, `deploy.resources.limits`, `init`,
`networks`, `dns`, `cap_add`, `cap_drop`, `devices`, `tmpfs`
(`/path:size=64m`), `sysctls`, `ulimits` and
`logging.options.max-size`. Unsupported keys are reported. Changes apply to
the form as you type.

## Verify

- The node's row on the **Overview** tab shows **Deploy** `deployed` and
  **State** `running`.
- **Logs** in the node's row opens a tab in the bottom panel with the service's output.
- **Operations → Jobs** lists one succeeded deploy job per node.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **Deploy** is disabled with "Save changes first" | The configuration has unsaved changes | Select **Save** or **Save and deploy** in the unsaved-changes bar |
| **Deploy** is disabled with "Choose nodes first" | No node is placed | Add a node under **Add node:** |
| Deploy fails with `… is a symlink or not a directory` or `… is not owned by root` | `<app>/.forge` was replaced by the app's user | Remove `<app>/.forge` as root and deploy again |
| Deploy fails with "the account names for `<environment>-<service>` (…) belong to accounts Orbyn Forge did not create" | Accounts with both the readable and the shortened name already exist on the node, without Forge's comment | Rename or remove those accounts on the node, or rename the app, then deploy again |
| Deploy fails with `usermod: user fa-… is currently used by process …` | A process outside the app's service still runs as the old account while Forge renames it | Stop that process, then deploy again |
| A job without systemd fails with "the app's account is missing" | The account was removed by hand | Deploy the app again |
| The app's folder or file root is missing after you placed the app | The `prepare` job failed (see **Operations → Jobs**), or the node's agent is too old for `prepare` (`unknown app action`) | Fix the cause or update the agent, then run the `prepare` action (`forge services action <service> prepare --node <node-name>`) or deploy |
| Deploy waits, then fails on a runtime | A runtime install failed on the node | Open the install job under **Packages → Installed**, fix the cause, deploy again |
| A runtime install fails with `Could not get lock /var/lib/dpkg/lock-frontend` | Another apt or dpkg run (outside Forge) held the lock for more than five minutes | Wait for that run to finish (`ps -C apt-get,dpkg`), then deploy again |
| A runtime install is refused | Installing needs `package.manage` with MFA | Verify with MFA, or ask an operator to deploy |
| An instance is `failed` with "stopped 5 times in 30 minutes" | The container keeps exiting, so Forge stopped restarting it | Read its logs, fix the cause, then **Start** or **Deploy** |
| An instance is `stranded` and did not move | It has local data, `on_node_failure` is `alert`, or no other eligible node exists | Bring the node back, or place the service elsewhere and remove the stranded instance |
| An exited container is not restarted | The node's agent does not report container states, or the person who last ran it was removed (`service.reconcile.refused` in the audit log) | Update the agent, or deploy once as an active user |
| A container deploy fails with `unauthorized` or "the registry `<host>` refused …" | The image is on a private registry that Forge has no credentials for, or the credentials cannot read it | Add or update the registry (see [Add a container registry](/docs/guides/registries/#troubleshooting)), then deploy again |
| A variable value is rejected | Values may not contain line breaks | Put multi-line content in a file and point the variable at it |

## Next steps

- [App Store](/docs/guides/app-store/) for runtimes and package variables.
- [Sites and DNS](/docs/guides/sites-and-dns/) to put a service behind a
  domain.
- [Cluster network](/docs/guides/cluster-network/) for internal names.
- [Deploy a compose stack](/docs/guides/compose-stacks/)
